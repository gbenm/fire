//! Validates YAML documents against `schemas/fire.schema.json`, so editors using the schema
//! don't flag files that fire accepts.

fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/fire.schema.json")).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

fn errors(yaml: &str) -> Vec<String> {
    let doc: serde_json::Value = yaml_serde::from_str(yaml).unwrap();
    validator()
        .iter_errors(&doc)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

#[test]
fn namespace_description_is_optional() {
    let errs = errors("namespace:\n  prefix: tools\ncommands:\n  hi: echo hi\n");
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn namespace_accepts_prefix_string() {
    let errs = errors("namespace: tools\ncommands:\n  hi: echo hi\n");
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn command_dependencies_and_parallel_validate() {
    let errs = errors(
        "commands:\n  lint: cargo clippy\n  test:\n    needs: [lint]\n    parallel: true\n    exec: cargo test\n",
    );
    assert!(errs.is_empty(), "{errs:?}");
    assert!(!errors(
        "commands:\n  test:\n    needs: lint\n    parallel: yes\n    exec: cargo test\n"
    )
    .is_empty());
}

#[test]
fn namespace_prefix_is_required() {
    let errs = errors("namespace:\n  description: Tools\ncommands:\n  hi: echo hi\n");
    assert!(!errs.is_empty());
}

#[test]
fn repo_fire_files_validate() {
    for path in [
        "config.sample.fire.yml",
        "infer.fire.yml",
        "samples/test.fire.yml",
    ] {
        let yaml = std::fs::read_to_string(path).unwrap();
        let errs = errors(&yaml);
        assert!(errs.is_empty(), "{path}: {errs:?}");
    }
}

#[test]
fn runner_accepts_a_container_object() {
    let yaml = r#"runtimes:
  ts:
    sdk: deno
    fallback_runner:
      image: denoland/deno:latest
      network_mode: host
commands:
  build:
    runner:
      image: node:lts-alpine
      engine: podman
      tty: true
      command: sh -c {fire.exec}
      volumes: [./:/app]
      working_dir: /app
      environment:
        NODE_ENV: production
      user: $(id -u):$(id -g)
      network: my-net
      args: [--cpus, "2"]
    exec: npm run build
"#;
    let errs = errors(yaml);
    assert!(errs.is_empty(), "{errs:?}");
}

#[test]
fn container_object_needs_an_image_and_known_keys() {
    assert!(!errors("commands:\n  a:\n    runner:\n      command: sh\n    exec: x\n").is_empty());
    assert!(!errors(
        "commands:\n  a:\n    runner:\n      image: alpine\n      imagen: x\n    exec: x\n"
    )
    .is_empty());
}
