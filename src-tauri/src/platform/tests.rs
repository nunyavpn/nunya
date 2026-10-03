use super::*;

/// A test binary stands where a development build does: not named `Nunya`, beside no installed
/// core. Offering the grant there would put network rights on a core anything local could drive.
#[test]
fn a_core_outside_an_installed_package_is_not_offered_the_grant() {
    let core = std::env::current_exe().unwrap().with_file_name("nunya-core");
    let why = grant_blocked(&core).expect("refused");
    assert!(why.contains("beside Nunya"), "{why}");
    assert!(grant(&core).is_err());
}
