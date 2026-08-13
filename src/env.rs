use std::{collections::BTreeMap, fs, path::Path};

use crate::config::EnvFileSetting;

pub(crate) fn resolve(
    config_dir: &Path,
    include_base_dir: Option<&Path>,
    setting: &EnvFileSetting,
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    let shell: BTreeMap<_, _> = std::env::vars().collect();
    let mut values = BTreeMap::new();

    match setting {
        EnvFileSetting::Null => {}
        EnvFileSetting::Unset => {
            if let Some(base) = include_base_dir {
                load_file(&base.join(".env"), &shell, &mut values, true)?;
            }
            load_file(&config_dir.join(".env"), &shell, &mut values, true)?;
        }
        EnvFileSetting::Paths(paths) => {
            for path in paths {
                load_file(&config_dir.join(path), &shell, &mut values, false)?;
            }
        }
    }

    for (key, value) in environment {
        let expanded = interpolate(value, &shell, &values)?;
        values.insert(key.clone(), expanded);
    }
    for (key, value) in shell {
        values.insert(key, value);
    }
    Ok(values)
}

fn load_file(
    path: &Path,
    shell: &BTreeMap<String, String>,
    values: &mut BTreeMap<String, String>,
    optional: bool,
) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && optional => return Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("[fire] env_file not found: {}. Continuing.", path.display());
            return Ok(());
        }
        Err(err) => return Err(format!("Could not read env_file {}: {err}", path.display())),
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let raw = raw.trim();
        let raw = raw
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| raw.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(raw);
        let expanded = interpolate(raw, shell, values)?;
        values.insert(key.to_string(), expanded);
    }
    Ok(())
}

fn interpolate(
    value: &str,
    shell: &BTreeMap<String, String>,
    values: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let expr = &after[..end];
        let (name, mode) = if let Some((name, default)) = expr.split_once(":-") {
            (name, Some((false, default)))
        } else if let Some((name, message)) = expr.split_once(":?") {
            (name, Some((true, message)))
        } else {
            (expr, None)
        };
        let current = shell
            .get(name)
            .or_else(|| values.get(name))
            .cloned()
            .unwrap_or_default();
        if let Some((required, fallback)) = mode {
            if current.is_empty() {
                if required {
                    return Err(if fallback.is_empty() {
                        format!("{name} is required")
                    } else {
                        fallback.to_string()
                    });
                }
                output.push_str(fallback);
            } else {
                output.push_str(&current);
            }
        } else {
            output.push_str(&current);
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expands_defaults_and_required_values() {
        let values = BTreeMap::from([("SET".to_string(), "yes".to_string())]);
        assert_eq!(
            interpolate("${SET}-${EMPTY:-fallback}", &BTreeMap::new(), &values).unwrap(),
            "yes-fallback"
        );
        assert_eq!(
            interpolate("${EMPTY:?missing EMPTY}", &BTreeMap::new(), &values).unwrap_err(),
            "missing EMPTY"
        );
    }

    #[test]
    fn included_auto_env_merges_base_then_own_and_inline_wins() {
        let root = std::env::temp_dir().join(format!("fire-env-{}", std::process::id()));
        let child = root.join("child");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join(".env"), "BASE=yes\nDUP=base\n").unwrap();
        fs::write(child.join(".env"), "CHILD=yes\nDUP=child\n").unwrap();
        let inline = BTreeMap::from([("DUP".to_string(), "${CHILD}-inline".to_string())]);

        let values = resolve(&child, Some(&root), &EnvFileSetting::Unset, &inline).unwrap();
        assert_eq!(values.get("BASE").map(String::as_str), Some("yes"));
        assert_eq!(values.get("CHILD").map(String::as_str), Some("yes"));
        assert_eq!(values.get("DUP").map(String::as_str), Some("yes-inline"));
        let _ = fs::remove_dir_all(root);
    }
}
