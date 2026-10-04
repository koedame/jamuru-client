#[path = "release_server_url.rs"]
mod release_server_url;

fn main() {
    check_release_server_url();
    tauri_build::build()
}

/// A release build asks the jamuru server given in `JAMJAM_SERVER_URL`
/// (`jamjam::config::RELEASE_SERVER_URL`) where its signaling server is.
/// Without it, or with one users could not reach, the build fails here rather
/// than shipping an app that cannot connect. The URL itself is not printed:
/// release logs are public.
fn check_release_server_url() {
    println!("cargo:rerun-if-env-changed=JAMJAM_SERVER_URL");
    println!("cargo:rerun-if-changed=release_server_url.rs");
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    let url = std::env::var("JAMJAM_SERVER_URL").unwrap_or_default();
    if let Some(problem) = release_server_url::release_server_url_problem(&url) {
        panic!("{}", problem);
    }
}
