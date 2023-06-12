use camino::Utf8PathBuf;

#[derive(Debug)]
pub struct AppArgs {
    pub config_file: Utf8PathBuf,
    pub probes_file: Utf8PathBuf,
}

const HELP: &str = r#"Tests uptime of KittyCAD API.
Usage: apimon [--config-file <path>] [--probe-file <path>]"#;

pub fn parse_args() -> Result<AppArgs, pico_args::Error> {
    let mut pargs = pico_args::Arguments::from_env();

    // Help has a higher priority and should be handled separately.
    if pargs.contains(["-h", "--help"]) {
        print!("{}", HELP);
        std::process::exit(0);
    }

    let args = AppArgs {
        config_file: pargs
            .opt_value_from_str("--config-file")?
            .unwrap_or("configuration/config.yaml".into()),
        probes_file: pargs
            .opt_value_from_str("--probes-file")?
            .unwrap_or("configuration/probes.yaml".into()),
    };

    Ok(args)
}
