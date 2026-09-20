use nunya_lib::config::Profile;

#[tokio::test]
#[ignore]
async fn probe() {
    let core = std::path::PathBuf::from(std::env::var("NUNYA_CORE_PATH").unwrap());
    let json = std::fs::read_to_string(std::env::var("PROFILES").unwrap()).unwrap();
    let profiles: Vec<Profile> = serde_json::from_str(&json).unwrap();
    let names: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();

    let started = std::time::Instant::now();
    match nunya_lib::geo::locate(&core, &profiles).await {
        Err(e) => println!("!!! {e}"),
        Ok(found) => {
            println!("=== located {} of {} in {:?} ===", found.len(), profiles.len(), started.elapsed());
            for f in &found {
                println!("{:<34} {:<4} {}", names[f.index], f.country, f.ip);
            }
        }
    }
}
