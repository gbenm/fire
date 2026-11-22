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
fn namespace_prefix_is_required() {
    let errs = errors("namespace:\n  description: Tools\ncommands:\n  hi: echo hi\n");
    assert!(!errs.is_empty());
}

#[test]
fn repo_fire_files_validate() {
    for path in ["config.sample.fire.yml", "infer.fire.yml", "samples/test.fire.yml"] {
        let yaml = std::fs::read_to_string(path).unwrap();
        let errs = errors(&yaml);
        assert!(errs.is_empty(), "{path}: {errs:?}");
    }
}
