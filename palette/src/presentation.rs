//! The palette's tree: nothing at all while it is closed, and while it is
//! open the whole window — an overlay that dismisses, and the card the reader
//! types in.
//!
//! The card is this view's, overlay included: what a palette looks like, where
//! it sits and what closes it are the same swap as what it finds.

use crate::{Message, PaletteView, host};
use ducktape_view_guest::{
    kit, slots,
    wire::{self, Length, Node},
};

/// How far below the window's top the card hangs.
const DROP: f32 = 96.;
/// The card's widest, and the height its results may take.
const CARD: f32 = 560.;
const RESULTS: f32 = 420.;

impl PaletteView {
    pub(crate) fn view(&self) -> Node {
        kit::set_dark(self.dark);
        // A CLOSED PALETTE IS AN EMPTY TREE, and an empty tree is what tells
        // the app there is no overlay to stack: nothing is drawn, nothing is
        // clicked through, and nothing is read.
        if !self.open {
            return kit::column("palette/closed", Vec::new());
        }
        let card = kit::aligned(
            kit::column(
                "palette/drop",
                [kit::space(None, Some(Length::Fixed(DROP))), self.card()],
            ),
            wire::AlignX::Center,
        );
        // AN OVERLAY, NOT A STACK OF TWO CHILDREN: the variant is the dialog
        // role, the label is the name read on open, and `on_dismiss` is the
        // door the host opens on Escape. A stack with a scrim button had
        // neither — the chord was the only way back out, which a reader who
        // cannot press it does not have.
        Node::Overlay {
            key: "palette/overlay".into(),
            label: Some("Command palette".into()),
            padding: 0.,
            // as the scrim was: the card carries the weight, and the layer
            // only catches the press that closes it
            backdrop: wire::Rgba([0.; 4]),
            align_x: wire::AlignX::Center,
            align_y: wire::AlignY::Top,
            on_dismiss: Some(slots::message(Message::Dismiss)),
            children: vec![kit::spacer(), card],
        }
    }

    fn card(&self) -> Node {
        let typed = slots::handler(Box::new(|text: String| Some(Message::DraftChanged(text))));
        // the named way out, beside the field: a dialog a pointer opened by
        // chord is one a pointer has to be able to close.
        let mut body = vec![kit::spaced(
            kit::row(
                "palette/bar",
                [
                    kit::input(
                        "palette/input",
                        "Search messages and pages",
                        self.draft.clone(),
                        typed,
                        None,
                    ),
                    kit::button(
                        "palette/close",
                        "Close",
                        Some(slots::message(Message::Dismiss)),
                        wire::ButtonPreset::Subtle,
                    ),
                ],
            ),
            kit::spacing::XS as f32,
        )];
        body.extend(self.search_state());
        body.push(self.hits());
        // the card fills a window narrower than it and stops at its width in
        // a wider one: a fixed width ran past the edge of a narrow window
        let mut card = kit::card(
            "palette/card",
            kit::spaced(kit::column("palette/body", body), kit::spacing::XS as f32),
        );
        if let Node::Container { max_width, .. } = &mut card {
            *max_width = Some(CARD);
        }
        card
    }

    /// What the field is doing, or nothing when the hits speak for
    /// themselves: the kit's empty state while it cannot answer or found
    /// nothing, a notice when the search failed.
    fn search_state(&self) -> Option<Node> {
        if !self.connected {
            return Some(kit::empty_state(
                "palette/disconnected",
                "Not connected",
                "Choose a network to search its messages and pages.",
            ));
        }
        if self.searching {
            return Some(kit::empty_state(
                "palette/searching",
                "Searching…",
                "Looking for matching messages and pages.",
            ));
        }
        if !self.error.is_empty() {
            return Some(kit::notice(
                "palette/error",
                kit::wrapping(kit::text("palette/error-text", &self.error)),
                kit::Tone::Danger,
            ));
        }
        let asked = !self.query.is_empty();
        let empty = self.chat.is_empty() && self.pages.is_empty();
        (asked && empty).then(|| {
            kit::empty_state(
                "palette/empty",
                "Nothing matched",
                "No messages or pages matched. Try fewer or different words.",
            )
        })
    }

    fn hits(&self) -> Node {
        let mut rows = Vec::new();
        for hit in &self.chat {
            rows.push(self.row(
                format!("palette/chat/{}/{}", hit.channel_id, hit.seq),
                hit.author.clone(),
                hit.text.clone(),
                host::chat_link(hit, &self.chain),
            ));
        }
        for hit in &self.pages {
            rows.push(self.row(
                format!("palette/page/{}/{}", hit.page_id, hit.block_id),
                hit.title.clone(),
                hit.text.clone(),
                host::page_link(hit, &self.chain),
            ));
        }
        if rows.is_empty() {
            return kit::column("palette/hits", Vec::new());
        }
        kit::sized(
            kit::scroll("palette/hits", kit::column("palette/rows", rows)),
            Some(Length::Fill),
            Some(Length::Fixed(RESULTS)),
        )
    }

    fn row(&self, key: String, name: String, detail: String, link: String) -> Node {
        let entry = kit::spaced(
            kit::column(
                key.clone(),
                [
                    kit::nowrap(kit::strong(format!("{key}/name"), name.clone())),
                    kit::nowrap(kit::secondary(format!("{key}/detail"), detail)),
                ],
            ),
            2.,
        );
        let mut press = kit::list_row(
            format!("{key}/press"),
            entry,
            false,
            Some(slots::message(Message::Open(link))),
        );
        if let Node::Button { label, .. } = &mut press {
            *label = Some(name);
        }
        press
    }
}
