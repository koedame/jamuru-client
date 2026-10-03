//! Keeps software rendering from starving the audio threads (Linux).
//!
//! Without a GPU the webview draws through Mesa's llvmpipe, which spreads
//! every frame over all cores. When the screen moves a few seconds after
//! joining a room, the audio threads cannot get a core and the sending side
//! stalls by several milliseconds at a time, which the other participants hear
//! as gaps. Drawing on one core and without the accelerated compositor leaves
//! the other cores to audio. The settings are inherited by the web process, so
//! they have to be in place before the first window is created.

/// Environment the web process reads for software rendering: one llvmpipe
/// thread, and no compositor thread behind it.
const SOFTWARE_RENDER_ENV: [(&str, &str); 2] = [
    ("LP_NUM_THREADS", "1"),
    ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
];

/// What to set for software rendering. Only for a machine where the webview
/// cannot reach a GPU, and never over a value the user has already set.
fn plan(gpu_available: bool, is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    if gpu_available {
        return Vec::new();
    }
    SOFTWARE_RENDER_ENV
        .into_iter()
        .filter(|(name, _)| !is_set(name))
        .collect()
}

/// Limits software rendering when there is no GPU to draw with. Returns what
/// it set, so that the caller can log it once the logger exists. Call it
/// before any thread is started and any window is created.
pub(crate) fn limit_software_rendering() -> Vec<(&'static str, &'static str)> {
    let limits = plan(gpu_render_node_usable(), |name| {
        std::env::var_os(name).is_some()
    });
    for (name, value) in &limits {
        std::env::set_var(name, value);
    }
    limits
}

/// Whether a GPU render node can be opened. Mesa falls back to llvmpipe when
/// there is none, or when the user may not open it (a virtual machine without
/// a virtual GPU, a container, a headless server).
#[cfg(target_os = "linux")]
fn gpu_render_node_usable() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
        .any(|entry| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path())
                .is_ok()
        })
}

/// Other platforms draw through their own compositor, so there is nothing to limit.
#[cfg(not(target_os = "linux"))]
fn gpu_render_node_usable() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_limits_both_when_there_is_no_gpu_and_nothing_is_set() {
        assert_eq!(
            plan(false, |_| false),
            vec![
                ("LP_NUM_THREADS", "1"),
                ("WEBKIT_DISABLE_COMPOSITING_MODE", "1")
            ]
        );
    }

    #[test]
    fn plan_leaves_rendering_alone_when_a_gpu_is_available() {
        assert!(plan(true, |_| false).is_empty());
    }

    #[test]
    fn plan_does_not_override_a_value_the_user_set() {
        assert_eq!(
            plan(false, |name| name == "LP_NUM_THREADS"),
            vec![("WEBKIT_DISABLE_COMPOSITING_MODE", "1")]
        );
    }
}
