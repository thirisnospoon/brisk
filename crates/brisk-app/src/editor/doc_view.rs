//! Documentation from ty drawn as PyCharm does: signatures and examples highlighted as
//! Python, prose with inline code, emphasis and clickable links. Shared by Quick
//! Documentation (Ctrl+Q / hover) and the completion list's documentation panel.

use crate::theme::{mono_font, ui_font, Theme};
use brisk_lsp::{DocBlock, InlineStyle};
use brisk_syntax::{Lang, SyntaxTree};
use gpui::prelude::*;
use gpui::{px, AnyElement, ElementId, FontStyle, FontWeight, HighlightStyle, InteractiveText, SharedString, StyledText, UnderlineStyle};
use std::cell::RefCell;

thread_local! {
    // Building the highlight query takes a moment; one parser serves every popup.
    static PYTHON: RefCell<Option<SyntaxTree>> = const { RefCell::new(None) };
}

/// Python highlighting of a code block, as `StyledText` ranges.
fn code_highlights(code: &str) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    PYTHON.with(|p| {
        let mut p = p.borrow_mut();
        let tree = p.get_or_insert_with(|| SyntaxTree::new(Lang::Python));
        tree.parse(code);
        tree.highlights(code, 0..code.len())
            .into_iter()
            .map(|s| (s.range, HighlightStyle { color: Some(Theme::syntax(s.kind)), ..Default::default() }))
            .collect()
    })
}

fn inline_style(s: &InlineStyle) -> HighlightStyle {
    match s {
        InlineStyle::Code => {
            HighlightStyle { color: Some(Theme::syntax(brisk_syntax::Highlight::String)), background_color: Some(Theme::hover_row()), ..Default::default() }
        }
        InlineStyle::Bold => HighlightStyle { font_weight: Some(FontWeight::BOLD), ..Default::default() },
        InlineStyle::Italic => HighlightStyle { font_style: Some(FontStyle::Italic), ..Default::default() },
        InlineStyle::Link(_) => HighlightStyle {
            color: Some(Theme::accent()),
            underline: Some(UnderlineStyle { thickness: px(1.), color: Some(Theme::accent()), wavy: false }),
            ..Default::default()
        },
    }
}

/// One paragraph of inline Markdown (`code`, **bold**, *italic*, links that open in the
/// browser) as text that wraps; `id` keeps its links apart from other paragraphs.
pub(crate) fn inline_markdown(md: &str, id: ElementId) -> InteractiveText {
    let d = brisk_lsp::parse_doc_text(md, true);
    let (mut ranges, mut urls) = (Vec::new(), Vec::new());
    for (r, s) in &d.styles {
        if let InlineStyle::Link(url) = s {
            ranges.push(r.clone());
            urls.push(url.clone());
        }
    }
    let hl = d.styles.iter().map(|(r, s)| (r.clone(), inline_style(s))).collect::<Vec<_>>();
    InteractiveText::new(id, StyledText::new(SharedString::from(d.text)).with_highlights(hl)).on_click(ranges, move |i, _, cx| cx.open_url(&urls[i]))
}

/// Elements for documentation blocks. `key` keeps the clickable texts' ids apart between
/// the popups; `compact` is the smaller type of the completion panel.
pub(super) fn doc_elements(blocks: &[DocBlock], key: &'static str, compact: bool) -> Vec<AnyElement> {
    let text_size = if compact { 12.5 } else { 13. };
    let code_size = if compact { 12. } else { 12.5 };
    let pad = if compact { 0. } else { 10. };
    blocks
        .iter()
        .enumerate()
        .map(|(ix, b)| match b {
            DocBlock::Code(code) => gpui::div()
                .px(px(pad))
                .py(px(if compact { 2. } else { 6. }))
                .font_family(mono_font())
                .text_size(px(code_size))
                .text_color(Theme::text())
                .whitespace_nowrap()
                .overflow_hidden()
                .child(StyledText::new(code.clone()).with_highlights(code_highlights(code)))
                .into_any_element(),
            DocBlock::Text(md) => {
                let d = brisk_lsp::parse_doc_text(md, compact);
                let links: Vec<(std::ops::Range<usize>, String)> = d
                    .styles
                    .iter()
                    .filter_map(|(r, s)| match s {
                        InlineStyle::Link(url) => Some((r.clone(), url.clone())),
                        _ => None,
                    })
                    .collect();
                let hl = d.styles.iter().map(|(r, s)| (r.clone(), inline_style(s))).collect::<Vec<_>>();
                let styled = StyledText::new(SharedString::from(d.text)).with_highlights(hl);
                let ranges = links.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
                let urls = links.into_iter().map(|(_, u)| u).collect::<Vec<_>>();
                let text = InteractiveText::new(ElementId::NamedInteger(key.into(), ix as u64), styled).on_click(ranges, move |i, _, cx| {
                    let url = &urls[i];
                    let url = if url.starts_with("www.") { format!("https://{url}") } else { url.clone() };
                    cx.open_url(&url);
                });
                gpui::div()
                    .px(px(pad))
                    .py(px(if compact { 2. } else { 6. }))
                    .font_family(ui_font())
                    .text_size(px(text_size))
                    .text_color(if compact { Theme::text_muted() } else { Theme::text() })
                    .child(text)
                    .into_any_element()
            }
            DocBlock::Rule => gpui::div().my(px(if compact { 3. } else { 0. })).h(px(1.)).bg(Theme::divider()).into_any_element(),
        })
        .collect()
}
