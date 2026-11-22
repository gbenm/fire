use std::{
    env,
    ffi::OsStr,
    io::{self, IsTerminal, Write},
    process::{self, Command, Stdio},
};

use rmcp::schemars;

pub(crate) struct Doc {
    pub(crate) id: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) content: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/embedded_docs.rs"));

pub(crate) fn all() -> &'static [Doc] {
    DOCS
}

pub(crate) fn find(id: &str) -> Option<&'static Doc> {
    DOCS.iter().find(|doc| doc.id == id)
}

pub(crate) fn index() -> String {
    DOCS.iter()
        .map(|doc| format!("{}\t{}", doc.id, doc.summary))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn handle_cli(topic: Option<&str>) {
    let Some(topic) = topic else {
        println!("{}", index());
        return;
    };
    let Some(doc) = find(topic) else {
        eprintln!("[fire] Unknown documentation topic `{topic}`.");
        eprintln!("Run `fire cli docs` to list available topics.");
        process::exit(1);
    };

    if io::stdout().is_terminal() {
        if let Some(pager) = env::var_os("PAGER").filter(|pager| !pager.is_empty()) {
            page(doc.content, &pager);
            return;
        }
    }
    print!("{}", doc.content);
}

/// `$PAGER` follows the conventional shell-command format, so arguments and quoting are kept for
/// the shell to interpret instead of being split or rewritten by Fire.
fn page(content: &str, pager: &OsStr) {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(pager)
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("[fire] Failed to start $PAGER: {err}");
            process::exit(1);
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(content.as_bytes()) {
            if err.kind() != io::ErrorKind::BrokenPipe {
                eprintln!("[fire] Failed to write documentation to $PAGER: {err}");
                let _ = child.kill();
                process::exit(1);
            }
        }
    }
    match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => process::exit(status.code().unwrap_or(1)),
        Err(err) => {
            eprintln!("[fire] Failed to wait for $PAGER: {err}");
            process::exit(1);
        }
    }
}
