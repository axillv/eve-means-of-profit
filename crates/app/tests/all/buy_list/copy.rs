use super::*;

/// Clicks Copy multibuy and returns the text it put on the clipboard.
pub(super) fn copy_multibuy(harness: &mut Harness<'_, App>) -> String {
    harness
        .get_all_by_label("Copy multibuy")
        .last()
        .expect("the copy button")
        .click();
    // The copy is made in the frame of the click, and `run` would leave only the last frame's output.
    harness.step();
    copied_text(harness).expect("the multibuy text")
}

/// Opens the pasted stock window from the Stock row, then pastes `text` into its box.
pub(super) fn paste(harness: &mut Harness<'_, App>, text: &str) {
    click(harness, "Pasted stock…");
    harness
        .get_by(|node| node.role() == Role::MultilineTextInput)
        .click();
    harness.event(Event::Paste(text.to_owned()));
    harness.run();
    harness.get_by_label("Close window").click();
    harness.run();
}

/// The text put on the clipboard by the frame just drawn, if any.
pub(super) fn copied_text(harness: &Harness<'_, App>) -> Option<String> {
    harness
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| {
            if let OutputCommand::CopyText(text) = command {
                Some(text.clone())
            } else {
                None
            }
        })
}

#[test]
fn copy_multibuy_puts_the_lines_on_the_clipboard() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    harness
        .get_all_by_label("Copy multibuy")
        .last()
        .expect("the copy button")
        .click();
    // The copy is made in the frame of the click, and `run` would leave only the last frame's output.
    harness.step();
    let copied = harness
        .output()
        .platform_output
        .commands
        .iter()
        .find_map(|command| {
            if let OutputCommand::CopyText(text) = command {
                Some(text.clone())
            } else {
                None
            }
        });
    assert_eq!(
        copied.as_deref(),
        Some(
            "Hobgoblin I\t10\nGuidance Systems\t10\nRobotics\t10\nMorphite\t10\nR.A.M.- Robotics\t10\nParticle Accelerator Unit\t10\nDatacore - Electronic Engineering\t3\nDatacore - Graviton Physics\t3"
        )
    );
    // The notice shows in the frame after the click: the status line is drawn before the page.
    harness.run();
    assert!(shown(&harness, "Copied 8 lines for multibuy."));
}

#[test]
fn copy_multibuy_is_disabled_when_every_line_is_in_stock() {
    let world = World::new();
    let mut harness = complete_list(&world, &standard_services());
    paste(
        &mut harness,
        "Morphite\t10\nParticle Accelerator Unit\t10\nGuidance Systems\t10\nHobgoblin I\t10\nRobotics\t10\nR.A.M.- Robotics\t10\nDatacore - Electronic Engineering\t3\nDatacore - Graviton Physics\t3",
    );
    harness.run();
    let copy = harness
        .get_all_by_label("Copy multibuy")
        .last()
        .expect("the copy button");
    assert!(copy.accesskit_node().is_disabled());
}
