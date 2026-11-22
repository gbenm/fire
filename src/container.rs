//! The container object for `runner` / `fallback_runner` (#44). It has no execution path of its
//! own: it only generates a string runner that follows the same rules as a hand-written one,
//! and that string is what fire runs, logs and shows in `--dry-run`.
//!
//! Values the user writes (`image`, `command`, `user`, `args`, ...) go into the string as
//! written, so the shell expands `$VAR`, `~` or `$(...)` in them. Values fire computes (paths)
//! are quoted. Environment values never go into the string: fire passes `-e KEY` and sets the
//! value in the runner's process environment, so secrets don't show up in logs.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use crate::{config::ContainerRunner, env, execute::shell_escape};

/// Where the generated runner runs.
pub(crate) struct Target<'a> {
    /// Mounted at the same absolute path when `volumes` is omitted.
    pub(crate) project_dir: &'a Path,
    /// Relative host paths in `volumes` resolve from here (the fire file's directory).
    pub(crate) config_dir: &'a Path,
    /// The command's resolved `dir`: default `working_dir`.
    pub(crate) dir: &'a Path,
    /// Variables the fire file declares (`environment`, `env_file`): forwarded by name.
    pub(crate) declared_env: &'a BTreeMap<String, String>,
    /// The full command environment, for `${VAR}` in the object's `environment`.
    pub(crate) environment: &'a BTreeMap<String, String>,
    /// Set for runtime runners.
    pub(crate) runtime: Option<RuntimeTarget<'a>>,
}

pub(crate) struct RuntimeTarget<'a> {
    /// The sdk's executable: default `command`.
    pub(crate) executable: &'a str,
    /// Files from the runtime's `paths`; their directories are mounted when outside the project.
    pub(crate) paths: &'a [PathBuf],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Generated {
    pub(crate) runner: String,
    /// Values for the `-e KEY` flags, set in the runner's process environment.
    pub(crate) env: BTreeMap<String, String>,
}

const FIRE_EXEC: &str = "{fire.exec}";

pub(crate) fn generate(
    container: &ContainerRunner,
    target: &Target<'_>,
) -> Result<Generated, String> {
    if let Some(key) = container.unknown.keys().next() {
        return Err(format!(
            "Container runner: unknown key `{key}`. Valid keys: image, engine, tty, command, volumes, working_dir, environment, user, network, network_mode, args."
        ));
    }
    let image = container.image.trim();
    if image.is_empty() {
        return Err("Container runner needs an `image`.".to_string());
    }
    let engine = match container.engine.as_deref().map(str::trim) {
        None | Some("") | Some("docker") => Engine::Docker,
        Some("podman") => Engine::Podman,
        Some(other) => {
            return Err(format!(
                "Container runner: unsupported engine `{other}`. Use `docker` or `podman`."
            ))
        }
    };
    let network = non_empty(&container.network);
    let network_mode = non_empty(&container.network_mode);
    if network.is_some() && network_mode.is_some() {
        return Err(
            "Container runner: `network` and `network_mode` can't be used together.".to_string(),
        );
    }
    let command = non_empty(&container.command);
    if container.tty
        && target.runtime.is_none()
        && !command.is_some_and(|command| command.contains(FIRE_EXEC))
    {
        return Err(format!(
            "Container runner: `tty: true` needs `{FIRE_EXEC}` in `command` (e.g. `command: sh -c {FIRE_EXEC}`). Without it fire writes the commands to stdin, which can't be a terminal."
        ));
    }

    let mut parts = vec![engine.program().to_string(), "run --rm -i".to_string()];
    if container.tty {
        parts.push("-t".to_string());
    }

    match &container.volumes {
        Some(volumes) => {
            for volume in volumes.iter().map(|volume| volume.trim()) {
                if !volume.is_empty() {
                    parts.push(format!("-v {}", explicit_volume(volume, target.config_dir)));
                }
            }
        }
        None => {
            for dir in inferred_mounts(target) {
                let dir = dir.to_string_lossy();
                parts.push(format!("-v {}", shell_escape(&format!("{dir}:{dir}"))));
            }
        }
    }

    match non_empty(&container.working_dir) {
        Some(dir) => parts.push(format!("-w {dir}")),
        None => parts.push(format!(
            "-w {}",
            shell_escape(&absolute(target.dir).to_string_lossy())
        )),
    }

    let mut env = BTreeMap::new();
    for (key, value) in &container.environment {
        let value = env::interpolate(value, &BTreeMap::new(), target.environment)?;
        env.insert(key.clone(), value);
    }
    let mut names = target.declared_env.keys().collect::<Vec<_>>();
    names.extend(container.environment.keys());
    names.sort();
    names.dedup();
    for name in names {
        parts.push(format!("-e {}", shell_escape(name)));
    }

    if let Some(user) = non_empty(&container.user) {
        parts.push(format!("--user {user}"));
    }
    if let Some(network) = network.or(network_mode) {
        parts.push(format!("--network {network}"));
    }

    if target.runtime.is_some() {
        let host = if network_mode == Some("host") {
            "127.0.0.1"
        } else {
            match engine {
                Engine::Docker => {
                    parts.push("--add-host=host.docker.internal:host-gateway".to_string());
                    "host.docker.internal"
                }
                Engine::Podman => "host.containers.internal",
            }
        };
        parts.push(format!(
            "-e FIRE_RUNTIME_RPC_ADDR={host}:$FIRE_RUNTIME_RPC_PORT"
        ));
    }

    parts.extend(
        container
            .args
            .iter()
            .map(|arg| arg.trim())
            .filter(|arg| !arg.is_empty())
            .map(ToOwned::to_owned),
    );
    parts.push(image.to_string());
    match (command, &target.runtime) {
        (Some(command), _) => parts.push(command.to_string()),
        (None, Some(runtime)) => parts.push(runtime.executable.to_string()),
        // the image's own command; fire writes the commands to its stdin
        (None, None) => {}
    }

    Ok(Generated {
        runner: parts.join(" "),
        env,
    })
}

#[derive(Debug, Clone, Copy)]
enum Engine {
    Docker,
    Podman,
}

impl Engine {
    fn program(self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
        }
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// `./src:/app` resolves the host side from the fire file's directory, as in Compose. Anything
/// else (absolute paths, `~/...`, named volumes) is left as written.
fn explicit_volume(volume: &str, config_dir: &Path) -> String {
    let (host, rest) = match volume.split_once(':') {
        Some((host, rest)) => (host, Some(rest)),
        None => (volume, None),
    };
    let relative = host == "." || host == ".." || host.starts_with("./") || host.starts_with("../");
    if !relative {
        return volume.to_string();
    }
    let host = normalize(&absolute(config_dir).join(host));
    let host = shell_escape(&host.to_string_lossy());
    match rest {
        Some(rest) => format!("{host}:{rest}"),
        None => host,
    }
}

/// The project directory, plus the command's `dir` and the directories of runtime `paths` that
/// live outside it, each mounted at the same absolute path.
fn inferred_mounts(target: &Target<'_>) -> Vec<PathBuf> {
    let mut candidates = vec![normalize(&absolute(target.project_dir))];
    candidates.push(normalize(&absolute(target.dir)));
    if let Some(runtime) = &target.runtime {
        for path in runtime.paths {
            if let Some(parent) = normalize(&absolute(path)).parent() {
                candidates.push(parent.to_path_buf());
            }
        }
    }
    let mut mounts: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if mounts.iter().any(|mount| candidate.starts_with(mount)) {
            continue;
        }
        mounts.retain(|mount| !mount.starts_with(&candidate));
        mounts.push(candidate);
    }
    mounts
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(yaml: &str) -> ContainerRunner {
        yaml_serde::from_str(yaml).unwrap()
    }

    fn target<'a>(
        declared: &'a BTreeMap<String, String>,
        runtime: Option<RuntimeTarget<'a>>,
    ) -> Target<'a> {
        Target {
            project_dir: Path::new("/proj"),
            config_dir: Path::new("/proj/sub"),
            dir: Path::new("/proj/web"),
            declared_env: declared,
            environment: declared,
            runtime,
        }
    }

    #[test]
    fn minimal_command_runner_uses_the_image_command() {
        let empty = BTreeMap::new();
        let generated = generate(&container("image: alpine"), &target(&empty, None)).unwrap();
        assert_eq!(
            generated.runner,
            "docker run --rm -i -v /proj:/proj -w /proj/web alpine"
        );
        assert!(generated.env.is_empty());
    }

    #[test]
    fn runtime_runner_defaults_to_the_sdk_executable_and_host_alias() {
        let empty = BTreeMap::new();
        let paths = [PathBuf::from("/lib/x/helpers.ts")];
        let runtime = RuntimeTarget {
            executable: "deno",
            paths: &paths,
        };
        let generated = generate(
            &container("image: denoland/deno:latest"),
            &target(&empty, Some(runtime)),
        )
        .unwrap();
        assert_eq!(
            generated.runner,
            "docker run --rm -i -v /proj:/proj -v /lib/x:/lib/x -w /proj/web --add-host=host.docker.internal:host-gateway -e FIRE_RUNTIME_RPC_ADDR=host.docker.internal:$FIRE_RUNTIME_RPC_PORT denoland/deno:latest deno"
        );
    }

    #[test]
    fn network_mode_host_uses_loopback_and_podman_its_alias() {
        let empty = BTreeMap::new();
        let runtime = || RuntimeTarget {
            executable: "node",
            paths: &[],
        };
        let host = generate(
            &container("image: node\nnetwork_mode: host"),
            &target(&empty, Some(runtime())),
        )
        .unwrap();
        assert!(
            host.runner.ends_with(
                "--network host -e FIRE_RUNTIME_RPC_ADDR=127.0.0.1:$FIRE_RUNTIME_RPC_PORT node node"
            ),
            "{}",
            host.runner
        );
        let podman = generate(
            &container("image: node\nengine: podman"),
            &target(&empty, Some(runtime())),
        )
        .unwrap();
        assert!(podman.runner.starts_with("podman run --rm -i "));
        assert!(podman.runner.contains(
            " -e FIRE_RUNTIME_RPC_ADDR=host.containers.internal:$FIRE_RUNTIME_RPC_PORT node node"
        ));
        assert!(!podman.runner.contains("--add-host"));
    }

    #[test]
    fn environment_goes_by_name_only() {
        let declared = BTreeMap::from([
            ("SECRET".to_string(), "s3cr3t".to_string()),
            ("NODE_ENV".to_string(), "dev".to_string()),
        ]);
        let generated = generate(
            &container("image: alpine\nenvironment:\n  NODE_ENV: production\n  TOKEN: ${SECRET}-x"),
            &target(&declared, None),
        )
        .unwrap();
        assert!(
            generated
                .runner
                .contains(" -e NODE_ENV -e SECRET -e TOKEN "),
            "{}",
            generated.runner
        );
        assert!(!generated.runner.contains("s3cr3t"));
        assert!(!generated.runner.contains("production"));
        assert_eq!(generated.env["NODE_ENV"], "production");
        assert_eq!(generated.env["TOKEN"], "s3cr3t-x");
    }

    #[test]
    fn explicit_keys_replace_the_defaults() {
        let empty = BTreeMap::new();
        let generated = generate(
            &container(
                "image: node:lts-alpine\ntty: true\ncommand: sh -c {fire.exec}\nvolumes: [./:/app, ~/.npm:/root/.npm, cache:/c]\nworking_dir: /app\nuser: $(id -u):$(id -g)\nnetwork: my-net\nargs: [--cpus, '2']",
            ),
            &target(&empty, None),
        )
        .unwrap();
        assert_eq!(
            generated.runner,
            "docker run --rm -i -t -v /proj/sub:/app -v ~/.npm:/root/.npm -v cache:/c -w /app --user $(id -u):$(id -g) --network my-net --cpus 2 node:lts-alpine sh -c {fire.exec}"
        );
    }

    #[test]
    fn invalid_combinations_are_errors() {
        let empty = BTreeMap::new();
        for (yaml, message) in [
            (
                "image: alpine\ntty: true",
                "`tty: true` needs `{fire.exec}` in `command`",
            ),
            (
                "image: alpine\ntty: true\ncommand: sh",
                "`tty: true` needs `{fire.exec}`",
            ),
            (
                "image: alpine\nnetwork: a\nnetwork_mode: host",
                "can't be used together",
            ),
            ("image: alpine\nengine: lxc", "unsupported engine `lxc`"),
            ("image: ''", "needs an `image`"),
            ("imagen: alpine", "unknown key `imagen`"),
        ] {
            let err = generate(&container(yaml), &target(&empty, None)).unwrap_err();
            assert!(err.contains(message), "{yaml}: {err}");
        }
    }
}
