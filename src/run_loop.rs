use std::{io::Cursor, sync::Arc, time::Duration};

use async_mutex::Mutex;
use camino::Utf8Path;
use futures::TryFutureExt;
use hyper::StatusCode;
use kittycad::types::{
    FailureWebSocketResponse, ModelingCmd, OkModelingCmdResponse, OkWebSocketResponseData,
    PathSegment, Point3D, RtcSessionDescription, SuccessWebSocketResponse, WebSocketRequest,
};
use slog::{o, Logger};
use tokio::{sync::Notify, time::error::Elapsed};
use tokio_tungstenite::tungstenite::Message as WsMsg;
use uuid::Uuid;
use webrtc::api::media_engine::MIME_TYPE_H264;
use webrtc::{
    data_channel::{data_channel_message::DataChannelMessage, RTCDataChannel},
    peer_connection,
    track::track_remote::TrackRemote,
};

use crate::probe::{Endpoint, Probe, ProbeMass};

pub async fn run_loop(client: kittycad::Client, config: crate::config::Config, logger: Logger) {
    let probes = config.probes;
    let n = probes.len();
    let client = Arc::new(client);
    loop {
        // Start all probes in parallel
        let mut handles = Vec::with_capacity(n);
        for probe in probes.clone() {
            let probe_client = client.clone();
            let name = probe.name.clone();
            let logger = logger.new(o!("probe_name" => name.clone()));
            let handle = tokio::spawn(async move {
                let _ = run_and_report(probe, probe_client, logger).await;
            });
            handles.push((name, handle));
        }

        // Wait for all probes.
        for (name, handle) in handles {
            if let Err(e) = handle.await {
                slog::error!(logger, "probe panicked"; "probe_name" => name, "panic" => ?e);
            }
        }

        slog::info!(logger, "sleeping"; "duration" => config.period.as_secs());
        tokio::time::sleep(config.period).await;
    }
}

/// Runs the probe and reports its results (via logs and metrics).
async fn run_and_report(probe: Probe, client: Arc<kittycad::Client>, logger: Logger) {
    let result = probe_endpoint(probe, client, logger.clone()).await;
    match result {
        Ok(()) => {
            slog::info!(logger, "probe succeeded");
        }
        Err(err) => {
            slog::error!(logger, "probe failed"; "error" => format!("{}", err));
        }
    }
}

/// Probe the specified API endpoint, check it returns the expected response.
/// The probe could fail because the API is unavailable, or because it gave an unexpected result.
/// If this returns OK, the endpoint is "healthy". Otherwise there's a problem.
pub async fn probe_endpoint(
    probe: Probe,
    client: Arc<kittycad::Client>,
    logger: Logger,
) -> Result<(), Error> {
    match probe.endpoint {
        Endpoint::FileMass { file_path, probe } => {
            let file = tokio::fs::read(file_path).await.unwrap();
            probe_file_mass(file, probe, client).await
        }
        Endpoint::Ping => probe_ping(client).await,
        Endpoint::ModelingWebsocket { img_output_path } => {
            probe_modeling_websocket(client, logger, &img_output_path).await
        }
    }
}

#[autometrics::autometrics]
async fn probe_ping(client: Arc<kittycad::Client>) -> Result<(), Error> {
    let _pong = client.meta().ping().or_else(wrap_kc).await?;
    Ok(())
}

#[autometrics::autometrics]
async fn probe_modeling_websocket(
    client: Arc<kittycad::Client>,
    log: Logger,
    img_output_path: &Utf8Path,
) -> Result<(), Error> {
    use futures::{SinkExt, StreamExt};

    let ws = client
        .modeling()
        .commands_ws(Some(30), Some(false), Some(480), Some(640), Some(false))
        .or_else(wrap_kc)
        .await?;
    let (mut write, mut read) = tokio_tungstenite::WebSocketStream::from_raw_socket(
        ws,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await
    .split();

    let to_msg = |cmd, cmd_id| {
        WsMsg::Text(
            serde_json::to_string(&WebSocketRequest::ModelingCmdReq { cmd, cmd_id }).unwrap(),
        )
    };

    // Start a path
    let path_id = Uuid::new_v4();
    write
        .send(to_msg(ModelingCmd::StartPath {}, path_id))
        .await
        .unwrap();

    const WIDTH: f64 = 10.0;
    // Draw the path in a square shape.
    let start = Point3D {
        x: -WIDTH,
        y: -WIDTH,
        z: -WIDTH,
    };

    write
        .send(to_msg(
            ModelingCmd::MovePathPen {
                path: path_id,
                to: start.clone(),
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();

    let points = [
        Point3D {
            x: WIDTH,
            y: -WIDTH,
            z: -WIDTH,
        },
        Point3D {
            x: WIDTH,
            y: WIDTH,
            z: -WIDTH,
        },
        Point3D {
            x: -WIDTH,
            y: WIDTH,
            z: -WIDTH,
        },
        start,
    ];
    for point in points {
        write
            .send(to_msg(
                ModelingCmd::ExtendPath {
                    path: path_id,
                    segment: PathSegment::Line {
                        end: point,
                        relative: false,
                    },
                },
                Uuid::new_v4(),
            ))
            .await
            .unwrap();
    }

    // Extrude the square into a cube.
    write
        .send(to_msg(ModelingCmd::ClosePath { path_id }, Uuid::new_v4()))
        .await
        .unwrap();
    write
        .send(to_msg(
            ModelingCmd::Extrude {
                cap: true,
                distance: WIDTH * 2.0,
                target: path_id,
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();
    write
        .send(to_msg(
            ModelingCmd::TakeSnapshot {
                format: kittycad::types::ImageFormat::Png,
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();

    // Finish sending
    drop(write);

    fn ws_resp_from_text(text: &str) -> Result<OkWebSocketResponseData, Error> {
        let resp: WebSocketResponse = serde_json::from_str(text)?;
        match resp {
            WebSocketResponse::Success(s) => {
                assert!(s.success);
                Ok(s.resp)
            }
            WebSocketResponse::Failure(mut f) => {
                assert!(!f.success);
                let Some(err) = f.errors.pop() else {
                    return Err(Error::UnexpectedApiResponse {
                        expected: "success = false means errors nonempty".to_owned(),
                        actual: "errors were empty".to_owned(),
                    });
                };
                Err(Error::UnexpectedApiResponse {
                    expected: "success only".to_owned(),
                    actual: format!("{err}"),
                })
            }
        }
    }

    fn text_from_ws(msg: WsMsg) -> Result<Option<String>, Error> {
        match msg {
            WsMsg::Text(text) => Ok(Some(text)),
            WsMsg::Pong(_) => Ok(None),
            other => Err(Error::UnexpectedApiResponse {
                expected: "only text responses".to_owned(),
                actual: format!("{other:?}"),
            }),
        }
    }

    // Get Websocket messages from API server
    let server_responses = async move {
        while let Some(msg) = read.next().await {
            let Some(resp) = text_from_ws(msg?)? else {
                continue;
            };
            let resp = ws_resp_from_text(&resp)?;
            match resp {
                OkWebSocketResponseData::Modeling { modeling_response } => {
                    match modeling_response {
                        OkModelingCmdResponse::Empty {} => {}
                        OkModelingCmdResponse::TakeSnapshot { data } => {
                            let mut img = image::io::Reader::new(Cursor::new(data.contents));
                            img.set_format(image::ImageFormat::Png);
                            let img = img.decode()?;
                            img.save(img_output_path)?;
                            break;
                        }
                        other => {
                            slog::debug!(log, "Got a websocket response"; "resp" => ?other)
                        }
                    }
                }
                _ => {
                    slog::debug!(log, "Got a websocket response"; "resp" => ?resp)
                }
            }
        }
        Ok::<_, Error>(())
    };
    tokio::time::timeout(Duration::from_secs(10), server_responses).await??;
    Ok(())
}

async fn assert_video_frames(track: Arc<TrackRemote>, closed: Arc<Notify>) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            result = track.read_rtp() => {
                if let Ok((rtp_packet, _)) = result {
                    println!("Received a frame!");
                }else{
                    println!("Couldn't find a frame.");
                    anyhow::bail!("Couldn't find a frame.");
                }
            }
            _ = closed.notified() => {
                println!("Video Closed!");
                return Ok(());
            }
        }
    }
}

#[autometrics::autometrics]
async fn probe_modeling_websocket_webrtc(
    client: Arc<kittycad::Client>,
    log: Logger,
    img_output_path: &Utf8Path,
) -> Result<(), Error> {
    use futures::{SinkExt, StreamExt};

    let ws = client
        .modeling()
        .commands_ws(Some(30), Some(false), Some(480), Some(640), Some(true))
        .or_else(wrap_kc)
        .await?;
    let (mut write, mut read) = tokio_tungstenite::WebSocketStream::from_raw_socket(
        ws,
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await
    .split();

    // Prepare the configuration.
    let global_ice_servers = Arc::new(Mutex::new(vec![]));
    let ice_servers = global_ice_servers.lock().await.clone();
    println!("ICE servers: {:?}", ice_servers);
    let config = webrtc::peer_connection::configuration::RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    // Create a MediaEngine object to configure the supported codec
    let mut m = webrtc::api::media_engine::MediaEngine::default();

    // Start the webrtc stuff.
    // Setup the codecs you want to use.
    m.register_codec(
        webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters {
            capability: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                mime_type: webrtc::api::media_engine::MIME_TYPE_H264.to_owned(),
                ..Default::default()
            },
            payload_type: 102,
            ..Default::default()
        },
        webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
    )?;

    let mut registry = webrtc::interceptor::registry::Registry::new();

    // Use the default set of Interceptors
    registry = webrtc::api::interceptor_registry::register_default_interceptors(registry, &mut m)?;

    // Create the API object with the MediaEngine
    let api = webrtc::api::APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    // Create a new RTCPeerConnection
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    // Allow us to receive 1 audio track, and 1 video track
    peer_connection
        .add_transceiver_from_kind(
            webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
            None,
        )
        .await?;

    // Continue setting up webRTC stuff.
    // Set the handler for ICE connection state
    // This will notify you when the peer has connected/disconnected
    peer_connection.on_ice_connection_state_change(Box::new(
        move |connection_state: webrtc::ice_transport::ice_connection_state::RTCIceConnectionState| {
            println!("Connection State has changed {connection_state}");

            if connection_state == webrtc::ice_transport::ice_connection_state::RTCIceConnectionState::Connected {
                println!("Ctrl+C the remote client to stop the demo");
            } else if connection_state == webrtc::ice_transport::ice_connection_state::RTCIceConnectionState::Failed {
                panic!("Connection failed");
            }
            Box::pin(async {})
        },
    ));

    let offer = peer_connection.create_offer(None).await?;
    // Send the offer back to the remote.
    let json_str = serde_json::to_string(&WebSocketRequest::SdpOffer {
        offer: RtcSessionDescription {
            sdp: offer.sdp.clone(),
            type_: match &offer.sdp_type {
                webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Unspecified => {
                    kittycad::types::RtcSdpType::Unspecified
                }
                webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Offer => {
                    kittycad::types::RtcSdpType::Offer
                }
                webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Pranswer => {
                    kittycad::types::RtcSdpType::Pranswer
                }
                webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Answer => {
                    kittycad::types::RtcSdpType::Answer
                }
                webrtc::peer_connection::sdp::sdp_type::RTCSdpType::Rollback => {
                    kittycad::types::RtcSdpType::Rollback
                }
            },
        },
    })?;
    write.send(WsMsg::Text(json_str)).await?;

    // Set the remote SessionDescription
    peer_connection.set_remote_description(offer).await?;

    // Create an answer
    let answer = peer_connection.create_answer(None).await?;

    // Create channel that is blocked until ICE Gathering is complete
    let mut gather_complete = peer_connection.gathering_complete_promise().await;

    // Sets the LocalDescription, and starts our UDP listeners
    peer_connection.set_local_description(answer).await?;

    // Block until ICE Gathering is complete, disabling trickle ICE
    // we do this because we only can exchange one signaling message
    // in a production application you should exchange ICE Candidates via OnICECandidate
    let _ = gather_complete.recv().await;

    if let Some(local_desc) = peer_connection.local_description().await {
        let _description = serde_json::to_string(&local_desc)?;
        println!("got local description");
    } else {
        panic!("getting local description failed");
    }

    // Get Video Track
    let notify_tx = Arc::new(Notify::new());
    let notify_rx = notify_tx.clone();
    peer_connection.on_track(Box::new(move |track, _, _| {
        let notify_rx2 = Arc::clone(&notify_rx);
        Box::pin(async move {
            let codec = track.codec();
            let mime_type = codec.capability.mime_type.to_lowercase();
            if mime_type == MIME_TYPE_H264.to_lowercase() {
                println!("Got h264 track, saving to disk as output.h264");
                tokio::spawn(async move {
                    let _ = assert_video_frames(track, notify_rx2).await;
                });
            } else {
                panic!("");
            }
        })
    }));

    let to_msg = |cmd, cmd_id| {
        WsMsg::Text(
            serde_json::to_string(&WebSocketRequest::ModelingCmdReq { cmd, cmd_id }).unwrap(),
        )
    };

    // Start a path
    let path_id = Uuid::new_v4();
    write
        .send(to_msg(ModelingCmd::StartPath {}, path_id))
        .await
        .unwrap();

    const WIDTH: f64 = 10.0;
    // Draw the path in a square shape.
    let start = Point3D {
        x: -WIDTH,
        y: -WIDTH,
        z: -WIDTH,
    };

    write
        .send(to_msg(
            ModelingCmd::MovePathPen {
                path: path_id,
                to: start.clone(),
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();

    let points = [
        Point3D {
            x: WIDTH,
            y: -WIDTH,
            z: -WIDTH,
        },
        Point3D {
            x: WIDTH,
            y: WIDTH,
            z: -WIDTH,
        },
        Point3D {
            x: -WIDTH,
            y: WIDTH,
            z: -WIDTH,
        },
        start,
    ];
    for point in points {
        write
            .send(to_msg(
                ModelingCmd::ExtendPath {
                    path: path_id,
                    segment: PathSegment::Line {
                        end: point,
                        relative: false,
                    },
                },
                Uuid::new_v4(),
            ))
            .await
            .unwrap();
    }

    // Extrude the square into a cube.
    write
        .send(to_msg(ModelingCmd::ClosePath { path_id }, Uuid::new_v4()))
        .await
        .unwrap();
    write
        .send(to_msg(
            ModelingCmd::Extrude {
                cap: true,
                distance: WIDTH * 2.0,
                target: path_id,
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();
    write
        .send(to_msg(
            ModelingCmd::TakeSnapshot {
                format: kittycad::types::ImageFormat::Png,
            },
            Uuid::new_v4(),
        ))
        .await
        .unwrap();

    // Finish sending
    drop(write);

    fn ws_resp_from_text(text: &str) -> Result<OkWebSocketResponseData, Error> {
        let resp: WebSocketResponse = serde_json::from_str(text)?;
        match resp {
            WebSocketResponse::Success(s) => {
                assert!(s.success);
                Ok(s.resp)
            }
            WebSocketResponse::Failure(mut f) => {
                assert!(!f.success);
                let Some(err) = f.errors.pop() else {
                    return Err(Error::UnexpectedApiResponse {
                        expected: "success = false means errors nonempty".to_owned(),
                        actual: "errors were empty".to_owned(),
                    });
                };
                Err(Error::UnexpectedApiResponse {
                    expected: "success only".to_owned(),
                    actual: format!("{err}"),
                })
            }
        }
    }

    fn text_from_ws(msg: WsMsg) -> Result<Option<String>, Error> {
        match msg {
            WsMsg::Text(text) => Ok(Some(text)),
            WsMsg::Pong(_) => Ok(None),
            other => Err(Error::UnexpectedApiResponse {
                expected: "only text responses".to_owned(),
                actual: format!("{other:?}"),
            }),
        }
    }

    // Get Websocket messages from API server
    let server_responses = async move {
        while let Some(msg) = read.next().await {
            let Some(resp) = text_from_ws(msg?)? else {
                continue;
            };
            let resp = ws_resp_from_text(&resp)?;
            match resp {
                OkWebSocketResponseData::Modeling { modeling_response } => {
                    match modeling_response {
                        OkModelingCmdResponse::Empty {} => {}
                        OkModelingCmdResponse::TakeSnapshot { data } => {
                            let mut img = image::io::Reader::new(Cursor::new(data.contents));
                            img.set_format(image::ImageFormat::Png);
                            let img = img.decode()?;
                            img.save(img_output_path)?;
                            break;
                        }
                        other => {
                            slog::debug!(log, "Got a websocket response"; "resp" => ?other)
                        }
                    }
                }
                _ => {
                    slog::debug!(log, "Got a websocket response"; "resp" => ?resp)
                }
            }
        }
        Ok::<_, Error>(())
    };
    tokio::time::timeout(Duration::from_secs(10), server_responses).await??;

    Ok(())
}

#[autometrics::autometrics]
async fn probe_file_mass(
    file: Vec<u8>,
    ProbeMass {
        src_format,
        material_density,
        material_density_unit,
        mass_unit,
        expected,
    }: ProbeMass,
    client: Arc<kittycad::Client>,
) -> Result<(), Error> {
    let resp = client
        .file()
        .create_mass(
            material_density,
            material_density_unit,
            mass_unit,
            src_format,
            &bytes::Bytes::from(file),
        )
        .or_else(wrap_kc)
        .await?;
    if expected.matches_actual(&resp) {
        Ok(())
    } else {
        Err(Error::UnexpectedApiResponse {
            expected: format!("{expected:?}"),
            actual: format!("{resp:?}"),
        })
    }
}

/// Different kinds of body that could come from a KittyCAD API client.
#[derive(Debug)]
pub enum Body {
    Json(serde_json::Value),
    Text(String),
    Binary,
    BodyError(reqwest::Error),
}

impl std::fmt::Display for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Json(c) => c.fmt(f),
            Body::Text(s) => s.fmt(f),
            Body::Binary => "[binary]".fmt(f),
            Body::BodyError(e) => e.fmt(f),
        }
    }
}

/// Errors the probe can detect.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("KittyCAD API client returned an unexpected response: HTTP {status:?}: {body}")]
    ApiClientUnexpectedResponse { status: StatusCode, body: Body },
    #[error("KittyCAD API client returned an error: {0}")]
    ApiClient(kittycad::types::error::Error),
    #[error("KittyCAD API sent {actual} but probe expected {expected}")]
    UnexpectedApiResponse { expected: String, actual: String },
    #[error("Error while reading from websocket connection with KittyCAD API: {0}")]
    CouldNotReadWebsocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Image error: {0}")]
    Img(#[from] image::ImageError),
    #[error("Websocket binary message did not deserialize into expected type")]
    WebsocketBinaryDeserialization(#[from] bincode::Error),
    #[error("Websocket text message did not deserialize into expected type")]
    WebsocketTextDeserialization(#[from] serde_json::Error),
    #[error("Timed out waiting for server to respond, in {0}")]
    WebsocketTimeout(#[from] Elapsed),
    #[error("WebRTC error: {0}")]
    WebRtcError(#[from] webrtc::Error),
}

/// Wrapper around `from_kc_err` to simplify callsites with .or_else method.
async fn wrap_kc<T>(e: kittycad::types::error::Error) -> Result<T, Error> {
    Err(Error::from_kc_err(e).await)
}

impl Error {
    // There are several different ways a KittyCAD API client error
    // could be mapped to Apimon errors. So simply putting a #[from]
    // wouldn't be enough.
    // And because this function is async, we can't just impl From,
    // which is inherently synchronous.
    async fn from_kc_err(e: kittycad::types::error::Error) -> Self {
        match e {
            kittycad::types::error::Error::UnexpectedResponse(r) => {
                // It actually takes a lot of work to get a useful readable body from this API.
                let status = r.status();
                let body = match r.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        return Self::ApiClientUnexpectedResponse {
                            status,
                            body: Body::BodyError(e),
                        }
                    }
                };
                let Ok(str) = std::str::from_utf8(&body) else {
                    return Self::ApiClientUnexpectedResponse {
                        status,
                        body: Body::Binary,
                    };
                };
                let Ok(json) = serde_json::from_str::<serde_json::Value>(str) else {
                    return Self::ApiClientUnexpectedResponse {
                        status,
                        body: Body::Text(str.to_owned()),
                    };
                };
                Self::ApiClientUnexpectedResponse {
                    status,
                    body: Body::Json(json),
                }
            }
            other => Self::ApiClient(other),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum WebSocketResponse {
    Success(SuccessWebSocketResponse),
    Failure(FailureWebSocketResponse),
}
