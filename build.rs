const PROMETHEUS_URL: &str = "http://prometheus-corp-virginia.hawk-dinosaur.ts.net:9090/";

fn main() {
    configure_autometrics();
}

fn configure_autometrics() {
    // Vergen sets environment variables that describe the current Git commit.
    // This lets Autometrics show if a commit has dramatically changed a metric.
    vergen::EmitBuilder::builder()
        .git_sha(true)
        .git_branch()
        .emit()
        .expect("Unable to generate build info");
    // Tell autometrics where to find the Prometheus server.
    println!("cargo:rustc-env=PROMETHEUS_URL={PROMETHEUS_URL}");
}
