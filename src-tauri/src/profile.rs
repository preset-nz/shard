//! Device profiles: what shard knows about a controller it talks *back* to.
//!
//! Learn-on-touch works because input is self-describing — a message arrives
//! and says what it is. Output is not. Nothing tells shard that CC 44 has a
//! light under it, which bytes make it green, or that the device must be put
//! into a mode before any of that works. So a device shard lights up needs a
//! profile, and a device it only listens to does not.
//!
//! This is the narrow end of `design/control-surface.md`: one profile, one
//! control, enough to prove the path from a button press to a light.
//!
//! Everything here was measured with `just probe`, not guessed. See
//! `design/controller-mapping.md` open question 2 for the wire detail.

/// A control the profile claims. The registry still learns it, so it appears
/// in Settings, but the map never sees it: the profile answers first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    pub channel: u8,
    pub cc: u8,
    /// The LED index, which on this device is the same as the CC number.
    pub led: u8,
    pub does: Does,
}

/// What a claimed control does. One entry today; gates and node switches are
/// the same shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Does {
    /// Start if stopped, stop if playing. The target decides what a press
    /// means, per decision 5 — the button carries no mode.
    TransportToggle,
}

pub struct Profile {
    pub name: &'static str,
    /// The input port name, which is how a device is recognised.
    pub input: &'static str,
    /// The port written to. **Not the same string**: this device is read as
    /// "LC3 1 MIDI Out" and written to as "LC3 1 DAW In", while an LPD8 is
    /// "LPD8" both ways. Pairing by name would have been wrong here.
    pub output: &'static str,
    /// Sent when the port opens, in order.
    pub on_connect: &'static [&'static [u8]],
    /// Sent when shard exits, returning the device to standalone.
    pub on_disconnect: &'static [&'static [u8]],
    pub claims: &'static [Claim],
}

impl Profile {
    /// The colour message for one control. RGB rather than the palette form:
    /// mixing the two left an encoder LED unresponsive once, and the cause
    /// was never established.
    pub fn light(&self, led: u8, rgb: (u8, u8, u8)) -> Vec<u8> {
        let (r, g, b) = rgb;
        vec![
            0xF0, 0x00, 0x20, 0x29, 0x02, 0x16, 0x01, 0x53, led, r, g, b, 0xF7,
        ]
    }

    pub fn claim(&self, channel: u8, cc: u8) -> Option<Claim> {
        self.claims
            .iter()
            .copied()
            .find(|c| c.channel == channel && c.cc == cc)
    }
}

/// Green while it plays, dim red while it does not. Read off the transport,
/// never off the press: the spacebar, a patch load and the sequencer all move
/// the transport, and a light driven by the button lies the first time one of
/// them does.
pub const PLAYING: (u8, u8, u8) = (0x00, 0x7F, 0x10);
pub const STOPPED: (u8, u8, u8) = (0x20, 0x00, 0x00);

/// Enable DAW mode. Without this the device answers nothing: no LEDs, no
/// screen, and the DAW port stays silent.
const DAW_ON: &[u8] = &[0xF0, 0x00, 0x20, 0x29, 0x02, 0x16, 0x02, 0x7F, 0xF7];
const DAW_OFF: &[u8] = &[0xF0, 0x00, 0x20, 0x29, 0x02, 0x16, 0x02, 0x00, 0xF7];
/// Relative output, per row, on channel 7. The CC numbers then shift by
/// +0x40, which is why the claims below are on buttons and not encoders.
const RELATIVE_ROW_1: &[u8] = &[0xB6, 0x45, 0x7F];
const RELATIVE_ROW_2: &[u8] = &[0xB6, 0x48, 0x7F];

/// Novation Launch Control 3: 16 endless encoders, 8 buttons, a screen.
pub const LAUNCH_CONTROL_3: Profile = Profile {
    name: "Launch Control 3",
    input: "LC3 1 DAW Out",
    output: "LC3 1 DAW In",
    on_connect: &[DAW_ON, RELATIVE_ROW_1, RELATIVE_ROW_2],
    on_disconnect: &[DAW_OFF],
    // Button 8. In DAW mode the buttons are channel 1, CC 37 to 44, and the
    // LED index is the CC number.
    claims: &[Claim {
        channel: 0,
        cc: 44,
        led: 44,
        does: Does::TransportToggle,
    }],
};

pub const PROFILES: &[Profile] = &[LAUNCH_CONTROL_3];

/// The profile for an input port, if shard has one.
pub fn for_input(port: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.input == port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_is_found_by_its_input_port_and_writes_to_another() {
        let p = for_input("LC3 1 DAW Out").expect("the LC3 is profiled");
        assert_eq!(p.output, "LC3 1 DAW In");
        // The trap this exists to record: the two names differ.
        assert_ne!(p.input, p.output);
        assert!(
            for_input("LPD8").is_none(),
            "the LPD8 is learned, not profiled"
        );
    }

    #[test]
    fn button_eight_is_claimed_and_nothing_else_is() {
        let p = for_input("LC3 1 DAW Out").unwrap();
        assert_eq!(p.claim(0, 44).map(|c| c.does), Some(Does::TransportToggle));
        assert!(p.claim(0, 43).is_none());
        // A knob on channel 16 is not a button on channel 1.
        assert!(p.claim(15, 44).is_none());
    }

    #[test]
    fn the_colour_message_is_the_measured_one() {
        let p = for_input("LC3 1 DAW Out").unwrap();
        assert_eq!(
            p.light(44, PLAYING),
            vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x16, 0x01, 0x53, 44, 0x00, 0x7F, 0x10, 0xF7]
        );
    }

    #[test]
    fn connecting_enables_daw_mode_and_disconnecting_undoes_it() {
        let p = for_input("LC3 1 DAW Out").unwrap();
        // Without DAW mode the device answers nothing, so it must come first.
        assert_eq!(p.on_connect.first().copied(), Some(DAW_ON));
        assert_eq!(p.on_disconnect, &[DAW_OFF]);
    }
}
