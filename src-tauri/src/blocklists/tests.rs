use super::*;

#[test]
fn a_hagezi_list_yields_its_names_and_nothing_else() {
    let text = "# Title: HaGeZi's Apple Tracker\n#\ncstat.g.aaplimg.com\n\n  Metrics.Example.com \n*.wild.example.net\nnot a name\n<html>\nlocalhost\n";
    assert_eq!(
        domains(text),
        ["cstat.g.aaplimg.com", "metrics.example.com", "wild.example.net"]
    );
}

#[test]
fn an_error_page_is_not_a_domain_list() {
    assert!(domains("<!DOCTYPE html><html><body>429 Too Many Requests</body></html>").is_empty());
}

#[test]
fn the_tracker_rule_set_blocks_each_name_and_everything_under_it() {
    let names: BTreeSet<String> = ["b.example.com", "a.example.com"].map(String::from).into();
    let set: Value = serde_json::from_slice(&tracker_rule_set(&names)).unwrap();
    assert_eq!(set["rules"][0]["domain_suffix"], json!(["a.example.com", "b.example.com"]));
}

#[test]
fn a_switch_whose_list_has_not_arrived_is_left_out() {
    let dir = std::env::temp_dir().join(format!("nunya-blocklists-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(DIR)).unwrap();
    fs::write(path(&dir, List::Trackers), "{}").unwrap();

    let both = BlockOptions { ads: true, trackers: true };
    let lists = on_disk(&dir, &both);
    assert_eq!(lists.len(), 1, "the ad list is not on disk");
    assert_eq!(lists[0].tag, "block-trackers");

    let none = BlockOptions::default();
    assert!(on_disk(&dir, &none).is_empty(), "a list on disk is not used while switched off");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn every_tracker_list_is_asked_for_once() {
    let unique: BTreeSet<&str> = TRACKER_URLS.into_iter().collect();
    assert_eq!(unique.len(), TRACKER_URLS.len());
}
