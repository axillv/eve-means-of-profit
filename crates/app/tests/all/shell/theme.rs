use super::*;

#[test]
fn the_colour_tokens_meet_the_contrast_values_of_the_design_note() {
    let style = Style::eve_dense();
    let ratio = |a, b| format!("{:.2}", contrast(a, b));
    assert_eq!(ratio(style.text, style.bg), "11.96");
    assert_eq!(ratio(style.text, style.panel), "11.17");
    assert_eq!(ratio(style.text, style.row), "10.21");
    assert_eq!(ratio(style.mute, style.bg), "5.54");
    assert_eq!(ratio(style.mute, style.row), "4.73");
    assert_eq!(ratio(style.on_accent, style.accent), "7.96");
    assert_eq!(ratio(style.stale, style.bg), "5.78");
    assert_eq!(ratio(style.pos, style.bg), "7.03");
    assert_eq!(ratio(style.neg, style.bg), "5.43");
    assert_eq!(ratio(style.row, style.line), "1.16");
    assert_eq!(ratio(style.bg, style.line), "1.36");
    assert_eq!(ratio(style.bg, style.row), "1.17");
    assert_eq!(ratio(style.bg, style.sel), "1.20");
}

#[test]
fn a_row_is_selected_then_hovered_then_zebra_then_plain() {
    let style = Style::eve_dense();
    assert_eq!(style.row_fill(true, true, true), style.sel);
    assert_eq!(style.row_fill(true, true, false), style.line);
    assert_eq!(style.row_fill(true, false, false), style.row);
    assert_eq!(style.row_fill(false, false, false), style.bg);
}
