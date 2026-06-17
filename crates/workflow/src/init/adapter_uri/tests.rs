use super::*;

#[test]
fn github_adapter_uri_parses_default_main() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia")
        .expect("parse GitHub URI");
    assert_eq!(
        parsed,
        GithubAdapterUri {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
            checkout_ref: None,
            adapter_path: "schemas/omnia".to_string(),
            adapter_name: "omnia".to_string(),
        }
    );
}

#[test]
fn github_adapter_uri_parses_suffix_ref() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/schemas/omnia@v1")
        .expect("parse GitHub URI");
    assert_eq!(parsed.checkout_ref.as_deref(), Some("v1"));
    assert_eq!(parsed.adapter_path, "schemas/omnia");
    assert_eq!(parsed.adapter_name, "omnia");
}

#[test]
fn github_adapter_uri_parses_tree_ref() {
    let parsed = GithubAdapterUri::parse("https://github.com/owner/repo/tree/main/schemas/omnia")
        .expect("parse GitHub URI");
    assert_eq!(parsed.checkout_ref.as_deref(), Some("main"));
    assert_eq!(parsed.adapter_path, "schemas/omnia");
    assert_eq!(parsed.adapter_name, "omnia");
}

#[test]
fn name_from_value_handles_shapes() {
    assert_eq!(adapter_name_from_value("omnia"), "omnia");
    assert_eq!(adapter_name_from_value("specify:omnia@1.2.0"), "omnia");
    assert_eq!(adapter_name_from_value("file:///abs/adapters/targets/omnia"), "omnia");
    assert_eq!(adapter_name_from_value("file:///abs/adapters/targets/omnia/"), "omnia");
    assert_eq!(
        adapter_name_from_value("https://github.com/augentic/specify/adapters/targets/omnia"),
        "omnia"
    );
    assert_eq!(
        adapter_name_from_value("https://github.com/augentic/specify/adapters/targets/omnia@v1"),
        "omnia"
    );
    assert_eq!(adapter_name_from_value("/abs/targets/omnia"), "omnia");
}

#[test]
fn package_ref_parses_namespace_name_and_semver() {
    let parsed = AdapterPackageRef::recognize("specify:omnia@1.2.0")
        .expect("recognised as a package reference")
        .expect("valid package reference");
    assert_eq!(
        parsed,
        AdapterPackageRef {
            namespace: "specify".to_string(),
            name: "omnia".to_string(),
            version: semver::Version::new(1, 2, 0),
        }
    );
    assert_eq!(parsed.wire_value(), "specify:omnia@1.2.0");
}

#[test]
fn package_ref_requires_exact_semver_never_a_branch() {
    // RFC-48 D2: an immutable locator pins an exact SemVer version. A
    // missing version, a git-style tag, or `latest` are all rejected —
    // there is no branch or tag defaulting.
    for malformed in ["specify:omnia", "specify:omnia@v1", "specify:omnia@1", "specify:omnia@latest"]
    {
        let result = AdapterPackageRef::recognize(malformed)
            .unwrap_or_else(|| panic!("`{malformed}` is a package-ref shape"));
        assert!(
            matches!(
                result,
                Err(Error::Diag { code: "adapter-package-ref-version-required", .. })
            ),
            "`{malformed}` must demand an exact SemVer pin",
        );
    }
}

#[test]
fn package_ref_recognises_only_package_shapes() {
    // URL schemes, drive paths, bare names, and local paths are not
    // package references — they keep flowing through the other branches.
    for non_package in [
        "omnia",
        "omnia@1.0.0",
        "./omnia",
        "/abs/omnia",
        "file:///abs/omnia",
        "https://github.com/augentic/specify/adapters/targets/omnia",
        r"C:\adapters\omnia",
        "C:/adapters/omnia",
    ] {
        assert!(
            AdapterPackageRef::recognize(non_package).is_none(),
            "`{non_package}` must not be treated as a package reference",
        );
    }
}

#[test]
fn parse_routes_package_ref_to_immutable_locator() {
    // A package reference resolves to the immutable registry locator,
    // never a mutable git checkout or a local-path fallback — the
    // RFC-48 Step 4 transport is reported as not-yet-wired rather than
    // silently degrading.
    let err = AdapterUri::parse("specify:omnia@1.2.0", Path::new("/tmp"))
        .expect_err("package reference is not yet fetchable");
    assert!(matches!(
        err,
        Error::Diag { code: "adapter-package-transport-unavailable", .. }
    ));
}

#[test]
fn shorthand_splits_name_and_semver_pin() {
    // A bare name carries no pin (resolves the single installed
    // identity); a `name@<semver>` carries the RFC-47 version pin.
    assert_eq!(parse_first_party_shorthand("omnia"), Some(("omnia", None)));
    assert_eq!(
        parse_first_party_shorthand("omnia@1.0.0"),
        Some(("omnia", Some(semver::Version::new(1, 0, 0))))
    );
    assert_eq!(parse_first_party_shorthand("typescript"), Some(("typescript", None)));
    assert_eq!(
        parse_first_party_shorthand("typescript@2.3.1"),
        Some(("typescript", Some(semver::Version::new(2, 3, 1))))
    );
}

#[test]
fn first_party_repo_routes_extracted_adapters() {
    // Bundled adapters (a WASI extension) have extracted to
    // specify-adapters; prose-only adapters still resolve from the
    // platform repo during the topology transition (RFC-48 / RFC-49).
    assert_eq!(first_party_repo("contracts"), "specify-adapters");
    assert_eq!(first_party_repo("vectis"), "specify-adapters");
    assert_eq!(first_party_repo("omnia"), "specify");
    assert_eq!(first_party_repo("typescript"), "specify");
}

#[test]
fn shorthand_rejects_non_shorthand() {
    // Paths and URLs flow through from_local / from_github instead.
    assert_eq!(parse_first_party_shorthand("./omnia"), None);
    assert_eq!(parse_first_party_shorthand("/abs/omnia"), None);
    assert_eq!(parse_first_party_shorthand("file:///abs/omnia"), None);
    assert_eq!(
        parse_first_party_shorthand("https://github.com/augentic/specify/adapters/targets/omnia"),
        None
    );
    // Not kebab-case, or a `@suffix` that is not exact semver.
    assert_eq!(parse_first_party_shorthand("Omnia"), None);
    assert_eq!(parse_first_party_shorthand("-omnia"), None);
    assert_eq!(parse_first_party_shorthand("omnia@v1"), None);
    assert_eq!(parse_first_party_shorthand("omnia@1"), None);
    assert_eq!(parse_first_party_shorthand("omnia@latest"), None);
    assert_eq!(parse_first_party_shorthand("omnia@"), None);
    assert_eq!(parse_first_party_shorthand(""), None);
}

#[test]
fn ref_from_value_recovers_semver_pin() {
    // A semver `@suffix` is recovered as a version pin; a bare name,
    // a `file://` path, and a non-semver git ref all yield `None`.
    assert_eq!(adapter_ref_from_value("omnia"), AdapterRef::bare("omnia"));
    assert_eq!(
        adapter_ref_from_value("omnia@1.0.0"),
        AdapterRef::pinned("omnia", semver::Version::new(1, 0, 0))
    );
    assert_eq!(adapter_ref_from_value("omnia@v1"), AdapterRef::bare("omnia"));
    assert_eq!(
        adapter_ref_from_value("file:///abs/adapters/targets/omnia"),
        AdapterRef::bare("omnia")
    );
    // A package reference recovers the bare `(name, version)` identity,
    // stripping the `<namespace>:` prefix.
    assert_eq!(
        adapter_ref_from_value("specify:omnia@1.2.0"),
        AdapterRef::pinned("omnia", semver::Version::new(1, 2, 0))
    );
}

#[test]
#[ignore = "networked GitHub fetch smoke test"]
fn shorthand_resolves_via_github() {
    // The shorthand resolves the canonical published first-party
    // adapter (a real sparse checkout of augentic/specify@v1).
    // Networked — run with `--ignored`.
    let parsed = AdapterUri::from_shorthand("omnia", Some(&semver::Version::new(1, 0, 0)))
        .expect("resolve shorthand against the published GitHub adapter");
    assert_eq!(parsed.adapter_name, "omnia");
}
