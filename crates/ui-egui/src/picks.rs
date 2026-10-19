//! File dialogs that answer later. iPadOS shows its document picker over the window and answers
//! through a delegate: a dialog run in line (a modal loop inside a frame) would re-enter winit's
//! event loop. Hosts with such pickers set [`Services::start_pick`](crate::Services::start_pick);
//! what asked for the file runs again with the path picked, when it arrives ([`poll`]).
//!
//! Hosts whose dialogs block ([`Services::pick_open`](crate::Services::pick_open) and
//! [`pick_save`](crate::Services::pick_save)) answer at once, as before.

use std::sync::mpsc::{Receiver, TryRecvError};

use serde_json::Value;

use crate::CadApp;

/// What a picker that answers later is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickRequest {
    /// A file to read.
    Open,
    /// Where to write a file with this suggested name.
    Save(String),
}

/// Show a picker for the request; the path picked (or `None` for Cancel) arrives on the
/// receiver. `None` when no picker could be shown.
pub type StartPick = Box<dyn Fn(PickRequest) -> Option<Receiver<Option<String>>>>;

type Then = Box<dyn FnOnce(&mut CadApp, Option<String>) -> Result<Value, String>>;

/// The picker open now, and what runs with its answer.
#[derive(Default)]
pub struct Pending(Option<(Receiver<Option<String>>, Then)>);

impl Pending {
    pub fn is_waiting(&self) -> bool {
        self.0.is_some()
    }
}

/// Whether the host can pick a file to open.
pub fn has_open(app: &CadApp) -> bool {
    app.services.pick_open.is_some() || app.services.start_pick.is_some()
}

/// Whether the host can pick where to save.
pub fn has_save(app: &CadApp) -> bool {
    app.services.pick_save.is_some() || app.services.start_pick.is_some()
}

/// Ask for a file to open; `then` runs with its path, now or when the picker answers. Cancel
/// runs nothing. `None` when the host has no picker; otherwise `then`'s result, or `Ok(Null)`
/// while the picker is open or after Cancel.
pub fn open(app: &mut CadApp, then: impl FnOnce(&mut CadApp, String) -> Result<Value, String> + 'static) -> Option<Result<Value, String>> {
    ask(app, PickRequest::Open, picked_only(then))
}

/// Ask where to save a file named `name` (suggested); as [`open`].
pub fn save(app: &mut CadApp, name: &str, then: impl FnOnce(&mut CadApp, String) -> Result<Value, String> + 'static) -> Option<Result<Value, String>> {
    ask(app, PickRequest::Save(name.to_string()), picked_only(then))
}

/// [`save`], also told about Cancel (`None`).
pub fn save_or_cancel(
    app: &mut CadApp,
    name: &str,
    then: impl FnOnce(&mut CadApp, Option<String>) -> Result<Value, String> + 'static,
) -> Option<Result<Value, String>> {
    ask(app, PickRequest::Save(name.to_string()), Box::new(then))
}

fn picked_only(then: impl FnOnce(&mut CadApp, String) -> Result<Value, String> + 'static) -> Then {
    Box::new(move |app, picked| match picked {
        Some(path) => then(app, path),
        None => Ok(Value::Null),
    })
}

fn ask(app: &mut CadApp, request: PickRequest, then: Then) -> Option<Result<Value, String>> {
    let now = match &request {
        PickRequest::Open => app.services.pick_open.as_ref().map(|pick| pick()),
        PickRequest::Save(name) => app.services.pick_save.as_ref().map(|pick| pick(name)),
    };
    if let Some(picked) = now {
        return Some(then(app, picked));
    }
    let start = app.services.start_pick.as_ref()?;
    // One picker at a time: a second request while one is open is dropped.
    if app.picks.is_waiting() {
        return Some(Ok(Value::Null));
    }
    match start(request) {
        Some(answer) => {
            app.picks.0 = Some((answer, then));
            Some(Ok(Value::Null))
        }
        None => Some(Err("The file picker could not be shown.".into())),
    }
}

/// Run what asked for a file once its picker has answered (called every frame).
pub fn poll(app: &mut CadApp) {
    let Some((answer, _)) = &app.picks.0 else { return };
    let picked = match answer.try_recv() {
        Ok(picked) => picked,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => None,
    };
    let Some((_, then)) = app.picks.0.take() else { return };
    if let Err(e) = then(app, picked) {
        app.set_status(e);
    }
}
