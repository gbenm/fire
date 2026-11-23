//! Repository-level acceptance coverage for the mdBook documentation site (#57).

use std::{fs, path::Path};

fn repo_file(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
}

#[test]
fn mdbook_is_configured_with_internal_link_checking() {
    let config = repo_file("book.toml");
    assert!(config.contains("src = \"docs\""), "{config}");
    assert!(config.contains("[output.html]"), "{config}");
    assert!(config.contains("[output.linkcheck2]"), "{config}");

    let summary = repo_file("docs/SUMMARY.md");
    assert!(summary.contains("[Introduction](./index.md)"), "{summary}");
    assert!(
        summary.contains("[Comparison](./comparison.md)"),
        "{summary}"
    );
}

#[test]
fn pages_workflow_builds_checks_and_deploys_on_main_and_releases() {
    let workflow = repo_file(".github/workflows/docs.yml");
    assert!(workflow.contains("branches: [main]"), "{workflow}");
    assert!(workflow.contains("release:"), "{workflow}");
    assert!(workflow.contains("mdbook-linkcheck"), "{workflow}");
    assert!(workflow.contains("mdbook build"), "{workflow}");
    assert!(workflow.contains("actions/deploy-pages@"), "{workflow}");
}

#[test]
fn public_site_is_linked_and_has_the_required_landing_content() {
    let readme = repo_file("README.md");
    assert!(readme.contains("https://gbenm.github.io/fire/"), "{readme}");

    let landing = repo_file("docs/index.md");
    assert!(landing.contains("./assets/demo.gif"), "{landing}");
    assert!(landing.contains("## Install"), "{landing}");
    assert!(landing.contains("## 30-second quickstart"), "{landing}");
    assert!(
        landing.contains("[comparison](./comparison.md)"),
        "{landing}"
    );
}
