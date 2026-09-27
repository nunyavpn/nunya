use super::*;

/// A Retina screen 1512pt wide: 3024 pixels.
const SCREEN: Option<(f64, f64)> = Some((0.0, 3024.0));

#[test]
fn the_panel_is_centred_under_its_icon_just_below_the_menu_bar() {
    let (x, y) = origin(2400.0, 68.0, 66.0, 2.0, SCREEN);
    let width = (PANEL_WIDTH + 2.0 * MARGIN) * 2.0;
    assert_eq!(x + width / 2.0, 2434.0, "centred on the icon's middle");
    // The panel itself, inside its transparent margin, starts a small gap below the bar.
    assert_eq!(y + MARGIN * 2.0, 66.0 + GAP * 2.0);
}

#[test]
fn an_icon_near_the_screens_edge_does_not_push_the_panel_off_it() {
    let width = (PANEL_WIDTH + 2.0 * MARGIN) * 2.0;
    let (right, _) = origin(2980.0, 40.0, 66.0, 2.0, SCREEN);
    assert_eq!(right + width, 3024.0);
    let (left, _) = origin(10.0, 40.0, 66.0, 2.0, SCREEN);
    assert_eq!(left, 0.0);
}

#[test]
fn a_screen_beside_the_first_is_measured_from_its_own_left_edge() {
    let (x, _) = origin(3100.0, 40.0, 66.0, 2.0, Some((3024.0, 6864.0)));
    assert!(x >= 3024.0, "the panel stayed on the icon's screen, at {x}");
}
