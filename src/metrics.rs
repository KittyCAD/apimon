use std::{convert::Infallible, net::SocketAddr};

use autometrics::prometheus_exporter::EncodingError;
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use slog::{error, Logger};

pub async fn serve_metrics(addr: SocketAddr, logger: Logger) {
    if let Err(e) = Server::bind(&addr)
        .serve(make_service_fn(|_conn| async {
            Ok::<_, Infallible>(service_fn(handle_metrics))
        }))
        .await
    {
        error!(logger, "metrics server failed"; "error" => ?e);
    }
}

/// Serves prometheus metrics
async fn handle_metrics(_req: Request<Body>) -> Result<Response<Body>, EncodingError> {
    let body = autometrics::prometheus_exporter::encode_to_string()?;
    Ok(Response::new(Body::from(body)))
}
