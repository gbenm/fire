use std::{
    env, fs,
    path::{Path, PathBuf},
};

struct SourceDoc {
    id: String,
    title: String,
    summary: String,
    path: PathBuf,
}

fn main() {
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=schemas/fire.schema.json");

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let mut docs = markdown_docs(&manifest.join("docs"));
    docs.push(schema_doc(&manifest.join("schemas/fire.schema.json")));
    docs.sort_by(|left, right| left.id.cmp(&right.id));

    let generated = generate(&docs);
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    fs::write(output.join("embedded_docs.rs"), generated).expect("write embedded docs module");
}

fn markdown_docs(directory: &Path) -> Vec<SourceDoc> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .expect("read docs directory")
        .map(|entry| entry.expect("read docs entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let content = fs::read_to_string(&path).expect("read documentation file");
            let id = path
                .file_stem()
                .and_then(|name| name.to_str())
                .expect("UTF-8 documentation file name")
                .to_string();
            let (title, summary) = markdown_metadata(&content, &path);
            SourceDoc {
                id,
                title,
                summary,
                path,
            }
        })
        .collect()
}

fn markdown_metadata(content: &str, path: &Path) -> (String, String) {
    let mut lines = content.lines();
    let title = lines
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| panic!("{} must start with a level-one title", path.display()))
        .to_string();

    let mut paragraph = Vec::new();
    for line in lines.skip_while(|line| line.trim().is_empty()) {
        if line.trim().is_empty() {
            break;
        }
        paragraph.push(line.trim());
    }
    assert!(
        !paragraph.is_empty(),
        "{} must have a summary paragraph after its title",
        path.display()
    );
    (title, paragraph.join(" "))
}

fn schema_doc(path: &Path) -> SourceDoc {
    let content = fs::read_to_string(path).expect("read Fire schema");
    let json: serde_json::Value = serde_json::from_str(&content).expect("parse Fire schema");
    let title = json["title"]
        .as_str()
        .expect("Fire schema needs a title")
        .to_string();
    let summary = json["description"]
        .as_str()
        .expect("Fire schema needs a description")
        .to_string();
    SourceDoc {
        id: "schema".to_string(),
        title,
        summary,
        path: path.to_path_buf(),
    }
}

fn generate(docs: &[SourceDoc]) -> String {
    let mut output = String::new();
    output.push_str("pub(crate) static DOCS: &[Doc] = &[\n");
    for doc in docs {
        output.push_str(&format!(
            "    Doc {{ id: {:?}, summary: {:?}, content: include_str!({:?}) }},\n",
            doc.id, doc.summary, doc.path
        ));
    }
    output.push_str("];\n\n");

    output.push_str("#[derive(Debug, Clone, Copy, serde::Deserialize, schemars::JsonSchema)]\n");
    output.push_str("pub(crate) enum DocsTopic {\n");
    for (index, doc) in docs.iter().enumerate() {
        output.push_str(&format!(
            "    #[serde(rename = {:?})]\n    Topic{index},\n",
            doc.id
        ));
    }
    output.push_str("}\n\nimpl DocsTopic {\n    pub(crate) fn id(self) -> &'static str {\n        match self {\n");
    for (index, doc) in docs.iter().enumerate() {
        output.push_str(&format!(
            "            Self::Topic{index} => {:?},\n",
            doc.id
        ));
    }
    output.push_str("        }\n    }\n}\n\n");

    let mut description = String::from(
        "Read Fire's version-matched embedded documentation. Omit `topic` to return the full topic index. Available topics:",
    );
    for doc in docs {
        description.push_str(&format!(
            "\n- `{}` ({}): {}",
            doc.id, doc.title, doc.summary
        ));
    }
    output.push_str(&format!(
        "pub(crate) const MCP_TOOL_DESCRIPTION: &str = {:?};\n",
        description
    ));
    output
}
