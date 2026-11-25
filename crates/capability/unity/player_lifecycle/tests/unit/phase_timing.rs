use super::*;

fn clock(mono: i64, cpu: i64) -> Clock { Clock { mono, cpu } }

fn sample(iteration: u64) -> Sample {
    Sample {
        iteration,
        start: clock(100, 10),
        input_end: clock(120, 18),
        render_end: clock(150, 38),
        scheduler_start: None,
        scheduler_end: None,
    }
}

#[test]
fn phases_preserve_clock_boundaries_and_include_actual_sleep() {
    let row = Row::complete(sample(64), clock(200, 44), Duration::from_nanos(40), Some(158)).unwrap();
    assert_eq!(row.iteration, 64);
    assert_eq!(row.start, 100);
    assert_eq!(row.input, (20, 8));
    assert_eq!(row.render, (30, 20));
    assert_eq!(row.post, (50, 6));
    assert_eq!((row.sleep_request, row.sleep_wall), (40, 42));
    let no_sleep = Row::complete(sample(64), clock(160, 40), Duration::ZERO, None).unwrap();
    assert_eq!((no_sleep.sleep_request, no_sleep.sleep_wall), (0, 0));
    assert!(Row::complete(sample(64), clock(99, 44), Duration::ZERO, None).is_none());
    assert!(Row::complete(sample(64), clock(200, 2), Duration::ZERO, None).is_none());
}

#[test]
fn trace_keeps_the_stride_and_capacity_and_refuses_overwrite() {
    let path = std::env::temp_dir().join(format!("rudroid-phase-test-{}.csv", std::process::id()));
    let mut trace = Trace {
        path: path.clone(),
        anchor: Anchor { before: 91, unix: 190, after: 93 },
        rows: Vec::new(),
        dropped: 0,
        invalid: 0,
        scheduler: None,
    };
    for iteration in 1..=MAX_SAMPLES as u64 * STRIDE {
        if trace.due(iteration) {
            trace.rows.push(Row::complete(sample(iteration), clock(200, 44), Duration::ZERO, None).unwrap());
        }
    }
    assert_eq!(trace.rows.len(), MAX_SAMPLES);
    assert!(!trace.due(MAX_SAMPLES as u64 * STRIDE + 1));
    assert!(!trace.due((MAX_SAMPLES as u64 + 1) * STRIDE));
    assert_eq!(trace.dropped, 1);
    drop(trace);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# anchor,start,91,190,93"));
    assert!(text.contains("samples=1024 dropped=1 invalid=0"));
    assert_eq!(text.lines().filter(|line| line.starts_with("64,")).count(), 1);
    let second = Trace {
        path: path.clone(),
        anchor: Anchor { before: 0, unix: 0, after: 0 },
        rows: Vec::new(),
        dropped: 0,
        invalid: 0,
        scheduler: None,
    };
    drop(second);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    std::fs::remove_file(path).unwrap();
}
