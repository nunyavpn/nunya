use super::*;

/// A test binary stands where a development build does: not named `Nunya`, beside no installed
/// core. Offering the grant there would put network rights on a core anything local could drive.
#[cfg(target_os = "linux")]
#[test]
fn a_core_outside_an_installed_package_is_not_offered_the_grant() {
    let core = std::env::current_exe().unwrap().with_file_name("nunya-core");
    let why = grant_blocked(&core).expect("refused");
    assert!(why.contains("beside Nunya"), "{why}");
    assert!(grant(&core).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn the_appimages_own_data_dirs_are_dropped_and_the_desktops_kept() {
    let dirs = "/tmp/.mount_NunyaX/usr/share:/usr/share:/usr/local/share::/tmp/.mount_NunyaX/usr/share";
    assert_eq!(
        imp::host_data_dirs(dirs, "/tmp/.mount_NunyaX/"),
        "/usr/share:/usr/local/share"
    );
}
