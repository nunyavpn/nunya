use super::*;

fn release(tag: &str, prerelease: bool, manifest: bool) -> Release {
    Release {
        tag_name: tag.into(),
        draft: false,
        prerelease,
        assets: if manifest {
            vec![Asset {
                name: MANIFEST.into(),
                browser_download_url: format!("https://example.net/{tag}/latest.json"),
            }]
        } else {
            vec![]
        },
    }
}

fn tag(found: Option<(&Release, &str)>) -> Option<String> {
    found.map(|(r, _)| r.tag_name.clone())
}

#[test]
fn betas_are_offered_only_to_those_who_asked_for_them() {
    let list = [release("v0.3.0", true, true), release("v0.2.0", false, true)];
    assert_eq!(tag(newest(&list, true)).as_deref(), Some("v0.3.0"));
    assert_eq!(tag(newest(&list, false)).as_deref(), Some("v0.2.0"));
}

#[test]
fn the_newest_is_chosen_by_version_not_by_order() {
    // A fix to an older line, published last, is listed first.
    let list = [release("v0.9.1", false, true), release("v0.10.0", false, true)];
    assert_eq!(tag(newest(&list, false)).as_deref(), Some("v0.10.0"));
}

#[test]
fn a_release_without_a_manifest_or_a_version_is_passed_over() {
    let mut draft = release("v0.5.0", false, true);
    draft.draft = true;
    let list = [
        draft,
        release("v0.4.0", false, false),
        release("nightly", false, true),
        release("v0.2.0", false, true),
    ];
    let (r, url) = newest(&list, true).unwrap();
    assert_eq!(r.tag_name, "v0.2.0");
    assert_eq!(url, "https://example.net/v0.2.0/latest.json");
}

#[test]
fn nothing_to_offer_is_none() {
    assert!(newest(&[release("v0.1.0", true, true)], false).is_none());
    assert!(newest(&[], true).is_none());
}

#[test]
fn github_s_release_list_is_read() {
    let body = r#"[{"tag_name":"v0.1.1","draft":false,"prerelease":true,"name":"Nunya 0.1.1",
        "assets":[{"name":"latest.json","browser_download_url":"https://github.com/x/latest.json","size":1}]}]"#;
    let list: Vec<Release> = serde_json::from_str(body).unwrap();
    assert_eq!(tag(newest(&list, true)).as_deref(), Some("v0.1.1"));
}

#[test]
#[ignore = "needs the internet"]
fn the_published_releases_are_listed() {
    let list = releases(None).unwrap();
    assert!(list.iter().any(|r| r.tag_name == "v0.1.0"), "{list:?}");
    // Releases before updates existed carry no manifest, so they are never offered.
    println!("offered: {:?}", tag(newest(&list, true)));
}
