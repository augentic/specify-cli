//! Integration coverage for `specify compatibility`.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::{TempDir, tempdir};

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn parse_json(stdout: &[u8]) -> Value {
    let text = std::str::from_utf8(stdout).expect("utf8 stdout");
    serde_json::from_str(text).unwrap_or_else(|err| panic!("stdout not JSON ({err}):\n{text}"))
}

struct Fixture {
    _tmp: TempDir,
    project: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempdir().expect("tempdir");
        let project = tmp.path().join("hub");
        write_file(
            &project.join(".specify/project.yaml"),
            "name: compatibility-hub\nhub: true\nrules: {}\n",
        );
        write_file(
            &project.join("registry.yaml"),
            "version: 1\nprojects:\n  - name: backend\n    url: ../backend\n    capability: omnia@v1\n    description: Backend API producer.\n    contracts:\n      produces:\n        - http/user-api.yaml\n  - name: mobile\n    url: ../mobile\n    capability: vectis@v1\n    description: Mobile API consumer.\n    contracts:\n      consumes:\n        - http/user-api.yaml\n",
        );
        Self { _tmp: tmp, project }
    }

    fn write_producer_contract(&self, body: &str) {
        write_file(&self.project.join("contracts/http/user-api.yaml"), body);
    }

    fn write_consumer_contract(&self, body: &str) {
        write_file(
            &self.project.join(".specify/workspace/mobile/contracts/http/user-api.yaml"),
            body,
        );
    }
}

#[test]
fn report_classifies_required_field_as_breaking() {
    let fixture = Fixture::new();
    fixture.write_consumer_contract(&openapi_contract(true, ""));
    fixture.write_producer_contract(&openapi_contract(true, "                - phone\n"));

    let assert = specify()
        .current_dir(&fixture.project)
        .args(["--format", "json", "compatibility", "report", "--change", "user-api-v2"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["change"], "user-api-v2");
    assert_eq!(value["checked-pairs"], 1);
    assert_eq!(value["ok"], false);
    assert!(value["summary"]["breaking"].as_u64().expect("breaking count") >= 1, "{value}");
    assert!(value["findings"].as_array().expect("findings").iter().any(|finding| {
        finding["classification"] == "breaking" && finding["change-kind"] == "required-field-added"
    }));
}

#[test]
fn check_succeeds_for_additive_optional_field() {
    let fixture = Fixture::new();
    fixture.write_consumer_contract(&openapi_contract(false, ""));
    fixture.write_producer_contract(&openapi_contract(true, ""));

    let assert = specify()
        .current_dir(&fixture.project)
        .args(["--format", "json", "compatibility", "check"])
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["ok"], true);
    assert!(value["summary"]["additive"].as_u64().expect("additive count") >= 1, "{value}");
}

fn openapi_contract(include_phone: bool, extra_required: &str) -> String {
    let phone_property =
        if include_phone { "                phone:\n                  type: string\n" } else { "" };
    format!(
        "openapi: '3.1.0'\ninfo:\n  title: User API\n  version: 1.0.0\n  x-specify-id: user-api\npaths:\n  /users:\n    post:\n      requestBody:\n        content:\n          application/json:\n            schema:\n              type: object\n              properties:\n                id:\n                  type: string\n{phone_property}              required:\n                - id\n{extra_required}      responses:\n        '200':\n          description: OK\n"
    )
}

fn write_file(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("mkdir");
    fs::write(path, content).expect("write file");
}
