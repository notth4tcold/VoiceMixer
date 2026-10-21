//! Single entry point for changing the mixer from outside the GUI: remote
//! socket, VBAN-TEXT, MIDI and macro buttons all go through here.

use crate::script::{self, Action, ParamKind, ScriptError, Value};
use crate::shared::Shared;
use crossbeam_channel::Sender;
use std::sync::Arc;

#[derive(Clone)]
pub struct Controller {
    pub shared: Arc<Shared>,
    actions: Sender<Action>,
}

impl Controller {
    pub fn new(shared: Arc<Shared>, actions: Sender<Action>) -> Self {
        Self { shared, actions }
    }

    /// Runs a script; parameter changes apply at once, actions are queued.
    pub fn run_script(&self, text: &str) -> Result<(), ScriptError> {
        let res = {
            let mut p = self.shared.params.lock();
            script::execute(&mut p, text)?
        };
        if res.changed {
            self.shared.bump();
        }
        if res.devices_changed {
            self.send(Action::ReconnectDevices);
        }
        if res.restart_needed {
            *self.shared.status.message.lock() =
                "Restart VoiceMixer to apply the new engine settings.".into();
        }
        for a in res.actions {
            self.send(a);
        }
        Ok(())
    }

    pub fn get(&self, path: &str) -> Result<Value, ScriptError> {
        script::get(&self.shared.params.lock(), path)
    }

    pub fn describe(&self, path: &str) -> Result<ParamKind, ScriptError> {
        script::describe(&self.shared.params.lock(), path)
    }

    pub fn send(&self, action: Action) {
        let _ = self.actions.send(action);
    }
}
