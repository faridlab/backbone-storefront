//! Gate 14: the module's exclusions. The events intake router is
//! absent from everything this module composes (no cargo edge, no
//! identifier, no route answers); the module carries ZERO references
//! to selling's alternate event-destination constructor or its sink
//! trait literal (the DIT #304 boundary: the storefront arms nothing).

use super::common::Probe;

/// Every `.rs` file under the module's `src/`, recursively.
fn source_files() -> Vec<std::path::PathBuf> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut out = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(format!("{manifest}/src"))];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    assert!(!out.is_empty(), "the source tree must exist to be scanned");
    out
}

#[test]
fn zero_banned_literals_in_the_module_source() {
    let banned = ["with_sink", "SellingEventSink"];
    for path in source_files() {
        let source = std::fs::read_to_string(&path).unwrap();
        for literal in banned {
            assert!(
                !source.contains(literal),
                "{literal} must not appear in {} — the module never arms an alternate event destination",
                path.display()
            );
        }
    }
}

#[test]
fn no_events_module_identifiers_in_the_module_source() {
    // The crate edge itself is absent (Cargo.toml carries no
    // backbone-events dependency); the source must agree — no intake
    // router mount, no events identifier anywhere.
    let banned = ["backbone_events", "event_intake", "intake_routes"];
    for path in source_files() {
        let source = std::fs::read_to_string(&path).unwrap();
        for literal in banned {
            assert!(
                !source.contains(literal),
                "{literal} must not appear in {} — the intake router stays unmounted",
                path.display()
            );
        }
    }
}

#[tokio::test]
async fn no_intake_route_answers_on_the_composed_routers() {
    let probe = Probe::boot("exclusions").await;
    // The events intake path shape (and near-miss variants) answer
    // 404 on the storefront's own routers — nothing is mounted.
    let paths = [
        "/public/events/intake",
        "/events/intake",
        "/api/v1/events/intake",
        "/public/intake",
    ];
    for path in paths {
        let (status, _) = super::common::get(&probe.public, path, None).await;
        assert_eq!(
            status,
            axum::http::StatusCode::NOT_FOUND,
            "GET {path} must be unrouted"
        );
    }
    probe.dispose().await;
}
