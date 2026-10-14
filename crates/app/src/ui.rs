//! egui rendering. Each submodule adds `impl App` blocks for one part of the window.

mod exclusion_ui;
mod icons;
mod menus;
mod preview_pane;
mod rename_editor;
mod search_bar;
mod status_bar;
pub(crate) mod table;
pub(crate) mod text_view;
mod windows_ui;

#[cfg(test)]
mod tests;
