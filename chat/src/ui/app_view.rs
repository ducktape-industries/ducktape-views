use super::*;
use ducktape_view_guest::slots;
impl super::ChatView {
    pub(crate) fn view(&self) -> wire::Node {
        native::set_dark(self.dark);
        let node_scope = format!("{}/chat", "ChatView");
        // Every press on the screen reports where it landed before the
        // control under it answers, so a menu opens at the pointer.
        let screen = wire::Node::MouseArea {
            key: format!("{node_scope}/press-area"),
            role: None,
            label: None,
            expanded: None,
            selected: None,
            checked: None,
            on_press: None,
            on_release: None,
            on_double_click: None,
            on_right_press: None,
            on_right_release: None,
            on_middle_press: None,
            on_middle_release: None,
            on_enter: None,
            on_exit: None,
            on_move: None,
            on_press_at: Some(slots::handler::<(f32, f32), Message>(Box::new(|(x, y)| {
                Some(Message::PressedAt(f64::from(x), f64::from(y)))
            }))),
            on_scroll: None,
            content: Box::new(self.chat_screen(node_scope.clone())),
        };
        let overlay = match self.floating_menu(&node_scope) {
            None => screen,
            Some(menu) => wire::Node::Overlay {
                key: format!("{node_scope}/menu-overlay"),
                label: Some("Message menu".into()),
                padding: 0.,
                backdrop: wire::Rgba([0.; 4]),
                align_x: wire::AlignX::Left,
                align_y: wire::AlignY::Top,
                on_dismiss: Some(slots::message(self.close_menu())),
                children: vec![screen, menu],
            },
        };
        // An open attachment previews in a modal card over everything: the
        // screen dims, and a press outside the card closes it.
        let overlay = match self.attachment_preview(&node_scope) {
            None => overlay,
            Some(card) => wire::Node::Overlay {
                key: format!("{node_scope}/preview-overlay"),
                label: Some("Attachment preview".into()),
                padding: 30.,
                backdrop: wire::Rgba([0., 0., 0., 0.55]),
                align_x: wire::AlignX::Center,
                align_y: wire::AlignY::Center,
                on_dismiss: Some(slots::message(Message::ClosePreview)),
                children: vec![overlay, card],
            },
        };
        let overlay = match self.channel_creation(&format!("{node_scope}/create")) {
            None => overlay,
            Some(card) => wire::Node::Overlay {
                key: format!("{node_scope}/create-overlay"),
                label: Some("Create channel".into()),
                padding: native::spacing::XL as f32,
                backdrop: wire::Rgba([0., 0., 0., 0.55]),
                align_x: wire::AlignX::Center,
                align_y: wire::AlignY::Center,
                on_dismiss: self
                    .channel_creating
                    .is_none()
                    .then(|| slots::message(Message::ToggleChannelCreate)),
                children: vec![overlay, card],
            },
        };
        wire::Node::Sensor {
            key: format!("{}/@sensor:906", "ChatView"),
            reset: None,
            on_show: Some(
                ::ducktape_view_guest::slots::handler::<(f32, f32), Message>(Box::new({
                    let route =
                        move |size: (f64, f64)| Message::ChatViewportChanged(size.0, size.1);
                    move |sent: (f32, f32)| Some(route((f64::from(sent.0), f64::from(sent.1))))
                })),
            ),
            on_resize: Some(
                ::ducktape_view_guest::slots::handler::<(f32, f32), Message>(Box::new({
                    let route =
                        move |size: (f64, f64)| Message::ChatViewportChanged(size.0, size.1);
                    move |sent: (f32, f32)| Some(route((f64::from(sent.0), f64::from(sent.1))))
                })),
            ),
            on_hide: None,
            anticipate: None,
            delay: None,
            child: Box::new(overlay),
        }
    }
}
