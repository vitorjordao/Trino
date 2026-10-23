//! The Steam backend (feature `steam`, issue #61): [`SteamStore`]
//! implements [`StoreApi`] over the `steamworks` crate — achievements,
//! INT stats, `StoreStats`, rich presence, the overlay's
//! `GameOverlayActivated` callback, Steam Cloud's state and Steam Input
//! (the action set `services export steam` writes:
//! `trino_scene::steam::input_manifest_vdf`).
//!
//! [`SteamStore::init`] fails without a Steam client (or a license): the
//! runner then keeps the local file backend and says why on stderr. The
//! game links `libsteam_api` dynamically; `cargo xtask package pc --steam`
//! puts it beside the executable. Never in `trino-core`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use steamworks::sys;
use trino_core::{Button, Vec2};

use crate::store::{StoreApi, StorePad};

/// Steam Input names, in `Button::ALL` order (the action manifest's).
const ACTIONS: [&str; 16] = [
    "button_a",
    "button_b",
    "button_x",
    "button_y",
    "button_l",
    "button_r",
    "button_start",
    "button_select",
    "dpad_up",
    "dpad_down",
    "dpad_left",
    "dpad_right",
    "button_l2",
    "button_r2",
    "button_l3",
    "button_r3",
];

struct SteamInput {
    set: sys::InputActionSetHandle_t,
    digital: [sys::InputDigitalActionHandle_t; 16],
    moves: sys::InputAnalogActionHandle_t,
    camera: sys::InputAnalogActionHandle_t,
    trigger_l: sys::InputAnalogActionHandle_t,
    trigger_r: sys::InputAnalogActionHandle_t,
    controller: Option<sys::InputHandle_t>,
}

pub struct SteamStore {
    client: steamworks::Client,
    overlay: Arc<AtomicBool>,
    _overlay_callback: steamworks::CallbackHandle,
    input: Option<SteamInput>,
}

impl SteamStore {
    /// Connect to the Steam client as `app_id`; `manifest` is the Steam
    /// Input action manifest to use (a development build; a released game's
    /// comes from Steamworks).
    pub fn init(app_id: u32, manifest: Option<&std::path::Path>) -> Result<SteamStore, String> {
        let client = steamworks::Client::init_app(app_id).map_err(|e| format!("{e}"))?;
        let overlay = Arc::new(AtomicBool::new(false));
        let flag = overlay.clone();
        let cb = client.register_callback(move |o: steamworks::GameOverlayActivated| {
            flag.store(o.active, Ordering::Relaxed);
        });
        let si = client.input();
        let input = si.init(false).then(|| {
            if let Some(m) = manifest.and_then(|m| m.to_str()) {
                si.set_input_action_manifest_file_path(m);
            }
            SteamInput {
                set: si.get_action_set_handle(trino_scene_action_set()),
                digital: ACTIONS.map(|a| si.get_digital_action_handle(a)),
                moves: si.get_analog_action_handle("move"),
                camera: si.get_analog_action_handle("camera"),
                trigger_l: si.get_analog_action_handle("trigger_l"),
                trigger_r: si.get_analog_action_handle("trigger_r"),
                controller: None,
            }
        });
        Ok(SteamStore {
            client,
            overlay,
            _overlay_callback: cb,
            input,
        })
    }

    /// Steam's own Deck check.
    pub fn on_steam_deck(&self) -> bool {
        self.client.utils().is_steam_running_on_steam_deck()
    }
}

/// The action set's name (`trino_scene::steam::ACTION_SET`).
fn trino_scene_action_set() -> &'static str {
    "InGameControls"
}

impl StoreApi for SteamStore {
    fn achievement(&self, api: &str) -> Option<bool> {
        self.client.user_stats().achievement(api).get().ok()
    }

    fn set_achievement(&mut self, api: &str) -> bool {
        self.client.user_stats().achievement(api).set().is_ok()
    }

    fn stat(&self, api: &str) -> Option<i32> {
        self.client.user_stats().get_stat_i32(api).ok()
    }

    fn set_stat(&mut self, api: &str, value: i32) -> bool {
        self.client.user_stats().set_stat_i32(api, value).is_ok()
    }

    fn store_stats(&mut self) -> bool {
        self.client.user_stats().store_stats().is_ok()
    }

    fn reset_all(&mut self) -> bool {
        self.client.user_stats().reset_all_stats(true).is_ok()
    }

    fn set_presence(&mut self, key: &str, value: Option<&str>) -> bool {
        self.client.friends().set_rich_presence(key, value)
    }

    fn clear_presence(&mut self) {
        self.client.friends().clear_rich_presence();
    }

    fn run_callbacks(&mut self) {
        self.client.run_callbacks();
    }

    fn overlay_open(&self) -> bool {
        self.overlay.load(Ordering::Relaxed)
    }

    fn cloud_enabled(&self) -> bool {
        let rs = self.client.remote_storage();
        rs.is_cloud_enabled_for_app() && rs.is_cloud_enabled_for_account()
    }

    fn pad(&mut self) -> Option<StorePad> {
        let input = self.input.as_mut()?;
        let si = self.client.input();
        let c = *si.get_connected_controllers().first()?;
        if input.controller != Some(c) {
            si.activate_action_set_handle(c, input.set);
            input.controller = Some(c);
        }
        let mut pad = StorePad::default();
        for (i, h) in input.digital.iter().enumerate() {
            let d = si.get_digital_action_data(c, *h);
            if d.bActive && d.bState {
                pad.buttons |= 1 << Button::ALL[i] as u32;
            }
        }
        let analog = |h| {
            let a = si.get_analog_action_data(c, h);
            if a.bActive {
                Vec2::new(a.x, a.y)
            } else {
                Vec2::ZERO
            }
        };
        pad.stick = analog(input.moves);
        pad.right_stick = analog(input.camera);
        pad.triggers = Vec2::new(analog(input.trigger_l).x, analog(input.trigger_r).x);
        Some(pad)
    }

    fn button_label(&self, button: Button) -> Option<String> {
        let input = self.input.as_ref()?;
        let c = input.controller?;
        let si = self.client.input();
        let h = input.digital[button as usize];
        let origin = *si.get_digital_action_origins(c, input.set, h).first()?;
        Some(si.get_string_for_action_origin(origin)).filter(|s| !s.is_empty())
    }
}
