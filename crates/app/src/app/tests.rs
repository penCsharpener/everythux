//! Shared fixtures for the state-machine tests.

use everythux_core::config::Config;
use everythux_core::ids::EntryId;

use super::App;
use crate::backend::fake::{FakeBackend, FakeHandle};
use crate::testkit::Fixture;

/// An [`App`] wired to a [`FakeBackend`] and the standard snapshot fixture.
pub(crate) struct TestApp {
    pub(crate) app: App,
    pub(crate) fake: FakeHandle,
    pub(crate) fixture: Fixture,
}

impl TestApp {
    pub(crate) fn new() -> Self {
        let (backend, fake, events) = FakeBackend::new();
        let app = App::new(
            egui::Context::default(),
            Box::new(backend),
            events,
            Config::default(),
            Vec::new(),
        );
        Self {
            app,
            fake,
            fixture: Fixture::new(),
        }
    }

    /// Delivers all pending backend events to the app.
    pub(crate) fn pump(&mut self) {
        self.app.poll_events();
    }

    /// Makes the app show exactly `ids` of the fixture snapshot as the current results.
    pub(crate) fn show_results(&mut self, ids: &[EntryId]) {
        let generation = self.app.search.latest_generation;
        self.fake
            .send_results(generation, &self.fixture.snapshot, ids, None);
        self.pump();
    }

    /// Shows every entry of the fixture.
    pub(crate) fn show_all_results(&mut self) {
        let all: Vec<EntryId> = (0..self.fixture.snapshot.len() as u32).map(EntryId).collect();
        self.show_results(&all);
    }
}
