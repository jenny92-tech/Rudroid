use super::*;

#[test]
fn private_trace_records_both_legs_at_stop() {
    for (name, route, status, readback) in [
        ("raw", Route::Raw, None, 0),
        ("sdl", Route::Sdl, Some(0), 1),
    ] {
        let dir =
            std::env::temp_dir().join(format!("rudroid-wayland-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let settings = Box::leak(Box::new(Settings {
            route,
            output: dir.join("probe.csv"),
        }));
        let trace = Trace::new(settings);
        trace.interval(1, 0, true, status, readback);
        trace.measure_swap(|| {});
        assert!(!settings.output.exists());
        trace.report().unwrap();
        trace.report().unwrap();
        let csv = std::fs::read_to_string(&settings.output).unwrap();
        assert!(csv.starts_with(&format!(
            "mode,{name}\nswap_samples,1,overwritten,0,interval_requests,1\n"
        )));
        let status = status.map(|value| value.to_string()).unwrap_or_default();
        assert!(csv.contains(&format!(",1,0,1,{status},{readback}\n")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
