//! What is this controller actually saying, and what does it do when you talk
//! back?
//!
//! Not part of the app. This is the tool the LPD8 table and the whole Launch
//! Control 3 protocol in `controller-mapping.md` were measured with, kept
//! because every controller so far has needed measuring and guessing has been
//! wrong every time. Three subcommands:
//!
//! ```text
//! just probe                        list input and output ports
//! just probe "listen 30"            print everything arriving, with its port
//! just probe "send <port> <hex..>"  send raw bytes, comma-separated per message
//! ```
//!
//! `send` takes messages as comma-separated hex, so a note on is `90,3C,7F`.
//! It prints what it sent, which matters when a device ignores something: you
//! want to know the bytes left as they were meant to.

use std::io::Write;

use midir::{Ignore, MidiInput, MidiOutput};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("listen") => listen(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20)),
        Some("send") => match args.get(2) {
            Some(port) => send(port, &parse(&args[3..])),
            None => eprintln!("usage: probe send <port> <hex,bytes> [<hex,bytes>...]"),
        },
        _ => ports(),
    }
}

/// Both directions. A device's input and output port names are not
/// necessarily the same string: the Launch Control 3 is read as
/// "LC3 1 MIDI Out" and written to as "LC3 1 MIDI In", while an LPD8 is
/// "LPD8" both ways.
fn ports() {
    if let Ok(i) = MidiInput::new("shard-probe") {
        println!("inputs:");
        for p in i.ports().iter() {
            println!("  {:?}", i.port_name(p).unwrap_or_default());
        }
    }
    if let Ok(o) = MidiOutput::new("shard-probe") {
        println!("outputs:");
        for p in o.ports().iter() {
            println!("  {:?}", o.port_name(p).unwrap_or_default());
        }
    }
}

/// Open every input and print what arrives. Clock and active sensing are
/// dropped because they drown everything else; SysEx is kept, because a
/// device's identity and its replies to queries arrive that way.
fn listen(seconds: u64) {
    let Ok(probe) = MidiInput::new("shard-probe") else {
        return;
    };
    let mut open = Vec::new();
    for p in probe.ports().iter() {
        let name = probe.port_name(p).unwrap_or_default();
        let Ok(mut input) = MidiInput::new("shard-probe") else {
            continue;
        };
        input.ignore(Ignore::TimeAndActiveSense);
        let port = name.clone();
        let conn = input.connect(
            p,
            "shard-probe",
            move |_, bytes, _| {
                println!("{port:<18} {}", describe(bytes));
                let _ = std::io::stdout().flush();
            },
            (),
        );
        match conn {
            Ok(c) => open.push(c),
            Err(e) => println!("could not open {name:?}: {e}"),
        }
    }
    println!("listening on {} port(s) for {seconds}s", open.len());
    let _ = std::io::stdout().flush();
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    println!("done");
}

fn describe(bytes: &[u8]) -> String {
    let (status, channel) = (bytes[0] >> 4, (bytes[0] & 0x0F) + 1);
    let kind = match status {
        0x8 => "note off",
        0x9 => "note on ",
        0xA => "aftertch",
        0xB => "cc      ",
        0xC => "program ",
        0xE => "bend    ",
        _ => "other   ",
    };
    let (a, b) = (
        bytes.get(1).copied().unwrap_or(0),
        bytes.get(2).copied().unwrap_or(0),
    );
    format!("ch{channel:<3}{kind} {a:<4}{b:<5}{bytes:02X?}")
}

/// Messages as comma-separated hex: `90,3C,7F`.
fn parse(args: &[String]) -> Vec<Vec<u8>> {
    args.iter()
        .map(|m| {
            m.split(',')
                .filter(|s| !s.is_empty())
                .map(|b| u8::from_str_radix(b.trim(), 16).expect("hex byte"))
                .collect()
        })
        .collect()
}

fn send(port: &str, messages: &[Vec<u8>]) {
    let Ok(out) = MidiOutput::new("shard-probe") else {
        return;
    };
    let found = out
        .ports()
        .iter()
        .find(|p| out.port_name(p).is_ok_and(|n| n == port))
        .cloned();
    let Some(p) = found else {
        eprintln!("no output port named {port:?}");
        ports();
        return;
    };
    let Ok(mut conn) = out.connect(&p, "shard-probe") else {
        eprintln!("could not open {port:?}");
        return;
    };
    for m in messages {
        println!("-> {port}  {m:02X?}");
        if let Err(e) = conn.send(m) {
            eprintln!("   failed: {e}");
        }
        // Slow enough that a device with a screen or LEDs can be watched.
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
}
