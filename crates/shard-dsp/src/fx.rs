//! The effect palette: kinds, a fixed pool of instances, and the order they run in.
//!
//! The process lane is a chain you build (`design/effect-palette.md`). A chain
//! owns a **pool**: `COPIES` instances of every kind, each with a stable number
//! `n` and its own rows, `fx.<n>.<kind>.<param>`. The **order** is sixteen
//! stepped rows, `fx.order.0` to `fx.order.15`, each naming an instance (0 is
//! empty, `v` is instance `v - 1`). An instance runs if the order names it.
//!
//! Why a pool and not a table rebuilt at runtime: parameter ids and indices
//! are held by the modulation links, the controller mappings, the patch file
//! and the UI. A pool never re-indexes, so a link to `fx.3.chorus.mix` is
//! still there after the chorus is taken out and put back, and removing an
//! effect keeps its settings.
//!
//! **Instance numbers are a wire format.** They come from `POOL`, a literal
//! list that only ever grows at the end. The order rows index the same list.

use std::sync::LazyLock;

use crate::fx_rows::{
    CHORUS_ROWS, CRUSH_ROWS, DELAY_ROWS, DRIVE_ROWS, ECHO_ROWS, FLANGER_ROWS, OVERTONE_ROWS,
    REVERB_ROWS, RING_ROWS, RISE_ROWS, WEAR_ROWS,
};
use crate::params::{ParamBank, ParamDef, Taper, Unit};

/// A kind of effect: a DSP module and the rows it brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Drive,
    Crush,
    Ring,
    Chorus,
    Overtone,
    Flanger,
    Wear,
    Delay,
    Echo,
    Rise,
    Reverb,
}

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Drive,
        Kind::Crush,
        Kind::Ring,
        Kind::Chorus,
        Kind::Overtone,
        Kind::Flanger,
        Kind::Wear,
        Kind::Delay,
        Kind::Echo,
        Kind::Rise,
        Kind::Reverb,
    ];

    /// The name in ids, and in the template rows: `chorus`.
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Drive => "drive",
            Kind::Crush => "crush",
            Kind::Ring => "ring",
            Kind::Chorus => "chorus",
            Kind::Overtone => "overtone",
            Kind::Flanger => "flanger",
            Kind::Wear => "wear",
            Kind::Delay => "delay",
            Kind::Echo => "echo",
            Kind::Rise => "rise",
            Kind::Reverb => "reverb",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// The template rows, ids as `<kind>.<param>`. `on` first, then `mix`.
    pub const fn rows(self) -> &'static [ParamDef] {
        match self {
            Kind::Drive => DRIVE_ROWS,
            Kind::Crush => CRUSH_ROWS,
            Kind::Ring => RING_ROWS,
            Kind::Chorus => CHORUS_ROWS,
            Kind::Overtone => OVERTONE_ROWS,
            Kind::Flanger => FLANGER_ROWS,
            Kind::Wear => WEAR_ROWS,
            Kind::Delay => DELAY_ROWS,
            Kind::Echo => ECHO_ROWS,
            Kind::Rise => RISE_ROWS,
            Kind::Reverb => REVERB_ROWS,
        }
    }

    /// What the section header says: the name of the kind's `on` row.
    pub fn label(self) -> &'static str {
        self.rows()[0].name
    }
}

/// How many of each kind a chain can hold at once.
pub const COPIES: usize = 2;
/// Every instance in the pool.
pub const INSTANCES: usize = 22;
/// How many effects a chain can run.
pub const ORDER_LEN: usize = 16;

/// The pool. **Append only.** An instance's number is its index here, and it
/// is baked into saved patches and controller maps. The first copy of every
/// kind comes first, then the second; a third copy or a twelfth kind goes on
/// the end and renumbers nothing.
pub const POOL: [(Kind, u8); INSTANCES] = [
    (Kind::Drive, 0),
    (Kind::Crush, 0),
    (Kind::Ring, 0),
    (Kind::Chorus, 0),
    (Kind::Overtone, 0),
    (Kind::Flanger, 0),
    (Kind::Wear, 0),
    (Kind::Delay, 0),
    (Kind::Echo, 0),
    (Kind::Rise, 0),
    (Kind::Reverb, 0),
    (Kind::Drive, 1),
    (Kind::Crush, 1),
    (Kind::Ring, 1),
    (Kind::Chorus, 1),
    (Kind::Overtone, 1),
    (Kind::Flanger, 1),
    (Kind::Wear, 1),
    (Kind::Delay, 1),
    (Kind::Echo, 1),
    (Kind::Rise, 1),
    (Kind::Reverb, 1),
];

pub fn kind_of(n: usize) -> Kind {
    POOL[n].0
}

/// The id of instance `n`'s row `template` (such as `chorus.mix`).
pub fn row_id(n: usize, template: &str) -> String {
    format!("fx.{n}.{template}")
}

/// `fx.3.chorus.mix` is instance 3 and `chorus.mix`. Any `arrangement.`
/// prefix is ignored. `None` for anything that is not an instance row.
pub fn parse_row(id: &str) -> Option<(usize, &str)> {
    let rest = id.strip_prefix(crate::arrangement::PREFIX).unwrap_or(id);
    let rest = rest.strip_prefix("fx.")?;
    let (n, template) = rest.split_once('.')?;
    let n: usize = n.parse().ok()?;
    (n < INSTANCES).then_some((n, template))
}

/// The id of order position `p`.
pub fn order_id(p: usize) -> String {
    format!("fx.order.{p}")
}

/// Whether `id` is one of the order rows, patch or arrangement. Never a
/// modulation target and never drawn as a panel.
pub fn is_order_row(id: &str) -> bool {
    let rest = id.strip_prefix(crate::arrangement::PREFIX).unwrap_or(id);
    rest.strip_prefix("fx.order.")
        .is_some_and(|p| p.parse::<usize>().is_ok_and(|p| p < ORDER_LEN))
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Every instance's rows, then the order rows. Built once at start, so the
/// ids can be `'static`; nothing is leaked after that.
pub(crate) fn rows() -> &'static [ParamDef] {
    static ROWS: LazyLock<Vec<ParamDef>> = LazyLock::new(|| {
        let mut out = Vec::new();
        for (n, (kind, _)) in POOL.iter().enumerate() {
            for row in kind.rows() {
                let mut def = *row;
                def.id = leak(row_id(n, row.id));
                // An added effect arrives on; `on` is a bypass from there.
                if row.id.ends_with(".on") {
                    def.default = 1.0;
                }
                out.push(def);
            }
        }
        for p in 0..ORDER_LEN {
            out.push(ParamDef {
                id: leak(order_id(p)),
                name: leak(format!("Position {}", p + 1)),
                min: 0.0,
                max: INSTANCES as f32,
                default: 0.0,
                taper: Taper::Stepped(INSTANCES as u32 + 1),
                unit: Unit::None,
                smooth_ms: 0.0,
            });
        }
        out
    });
    &ROWS
}

/// The chain's order, cleaned: only instances that exist, each once, with no
/// gaps. Built from raw stepped values, so a hand-edited file cannot name an
/// instance that is not there or run one twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Order {
    items: [u8; ORDER_LEN],
    len: u8,
}

impl Order {
    pub const EMPTY: Order = Order {
        items: [0; ORDER_LEN],
        len: 0,
    };

    /// From the order rows' values: 0 is empty, `v` is instance `v - 1`.
    pub fn sanitise(values: [f32; ORDER_LEN]) -> Order {
        let mut order = Order::EMPTY;
        let mut seen = [false; INSTANCES];
        for v in values {
            let v = if v.is_finite() { v.round() } else { 0.0 };
            if v < 1.0 || v > INSTANCES as f32 {
                continue;
            }
            let n = v as usize - 1;
            if seen[n] {
                continue;
            }
            seen[n] = true;
            order.items[order.len as usize] = n as u8;
            order.len += 1;
        }
        order
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The instance numbers, first to run first.
    pub fn as_slice(&self) -> &[u8] {
        &self.items[..self.len as usize]
    }

    pub fn contains(&self, n: usize) -> bool {
        self.as_slice().contains(&(n as u8))
    }

    /// Back to order-row values, padded with zeros.
    pub fn to_values(&self) -> [f32; ORDER_LEN] {
        let mut out = [0.0; ORDER_LEN];
        for (slot, n) in out.iter_mut().zip(self.as_slice()) {
            *slot = *n as f32 + 1.0;
        }
        out
    }

    /// Adds the first free copy of `kind` at the end. `None` if every copy is
    /// in the chain or the chain is full.
    pub fn add(&mut self, kind: Kind) -> Option<usize> {
        if self.len() >= ORDER_LEN {
            return None;
        }
        let n = (0..INSTANCES).find(|&n| kind_of(n) == kind && !self.contains(n))?;
        self.items[self.len as usize] = n as u8;
        self.len += 1;
        Some(n)
    }

    /// Puts instance `n` at the end, if it is not in the chain and there is
    /// room. Returns whether it did.
    pub fn push(&mut self, n: usize) -> bool {
        if n >= INSTANCES || self.contains(n) || self.len() >= ORDER_LEN {
            return false;
        }
        self.items[self.len as usize] = n as u8;
        self.len += 1;
        true
    }

    /// Takes instance `n` out. Returns whether it was there.
    pub fn remove(&mut self, n: usize) -> bool {
        let Some(at) = self.as_slice().iter().position(|&x| x as usize == n) else {
            return false;
        };
        self.items.copy_within(at + 1..self.len as usize, at);
        self.len -= 1;
        true
    }

    /// Moves instance `n` one place earlier (`by = -1`) or later (`by = 1`),
    /// stopping at the ends. Returns whether it moved.
    pub fn nudge(&mut self, n: usize, by: i32) -> bool {
        let Some(at) = self.as_slice().iter().position(|&x| x as usize == n) else {
            return false;
        };
        let to = at as i32 + by.signum();
        if to < 0 || to >= self.len() as i32 {
            return false;
        }
        self.items.swap(at, to as usize);
        true
    }
}

/// The prefix of the bank's order rows: empty for the patch's, `arrangement.`
/// for the arrangement's.
fn bank_prefix(bank: &ParamBank) -> &'static str {
    if bank.index(&order_id(0)).is_some() {
        ""
    } else {
        crate::arrangement::PREFIX
    }
}

/// The chain order a bank holds, cleaned.
pub fn order_of(bank: &ParamBank) -> Order {
    let prefix = bank_prefix(bank);
    let mut values = [0.0; ORDER_LEN];
    for (p, v) in values.iter_mut().enumerate() {
        *v = bank
            .get_by_id(&format!("{prefix}{}", order_id(p)))
            .unwrap_or(0.0);
    }
    Order::sanitise(values)
}

/// Writes an order into a bank's order rows.
pub fn set_order(bank: &ParamBank, order: &Order) {
    let prefix = bank_prefix(bank);
    for (p, v) in order.to_values().into_iter().enumerate() {
        bank.set_by_id(&format!("{prefix}{}", order_id(p)), v);
    }
}

/// Puts the first free copy of `kind` at the end of the chain a bank holds,
/// switched on. Its other settings are whatever it last had. Returns the
/// instance number.
pub fn add_to_chain(bank: &ParamBank, kind: Kind) -> Result<usize, &'static str> {
    let mut order = order_of(bank);
    let n = order.add(kind).ok_or(if order.len() >= ORDER_LEN {
        "the chain is full"
    } else {
        "both copies of that effect are already in the chain"
    })?;
    set_order(bank, &order);
    let prefix = bank_prefix(bank);
    bank.set_by_id(
        &format!("{prefix}{}", row_id(n, &format!("{}.on", kind.name()))),
        1.0,
    );
    Ok(n)
}

/// Takes instance `n` out of the chain a bank holds. Its settings stay.
pub fn remove_from_chain(bank: &ParamBank, n: usize) -> Result<(), &'static str> {
    let mut order = order_of(bank);
    if !order.remove(n) {
        return Err("that effect is not in the chain");
    }
    set_order(bank, &order);
    Ok(())
}

/// Moves instance `n` one place earlier (`by < 0`) or later (`by > 0`).
/// Returns whether it moved; at either end it does not.
pub fn move_in_chain(bank: &ParamBank, n: usize, by: i32) -> Result<bool, &'static str> {
    let mut order = order_of(bank);
    if !order.contains(n) {
        return Err("that effect is not in the chain");
    }
    let moved = order.nudge(n, by);
    if moved {
        set_order(bank, &order);
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_is_append_only() {
        // These numbers are in saved patches and controller maps. If this
        // test fails, an instance moved. Add to the end instead.
        assert_eq!(INSTANCES, POOL.len());
        let names: Vec<_> = POOL.iter().take(12).map(|(k, c)| (k.name(), *c)).collect();
        assert_eq!(
            names,
            vec![
                ("drive", 0),
                ("crush", 0),
                ("ring", 0),
                ("chorus", 0),
                ("overtone", 0),
                ("flanger", 0),
                ("wear", 0),
                ("delay", 0),
                ("echo", 0),
                ("rise", 0),
                ("reverb", 0),
                ("drive", 1),
            ]
        );
    }

    #[test]
    fn every_kind_has_the_same_number_of_copies() {
        for kind in Kind::ALL {
            assert_eq!(
                POOL.iter().filter(|(k, _)| *k == kind).count(),
                COPIES,
                "{}",
                kind.name()
            );
        }
    }

    #[test]
    fn ids_are_unique_and_parse_back() {
        let mut seen = std::collections::HashSet::new();
        for def in rows() {
            assert!(seen.insert(def.id), "duplicate {}", def.id);
            if !is_order_row(def.id) {
                let (n, template) = parse_row(def.id).expect(def.id);
                assert!(template.starts_with(kind_of(n).name()), "{}", def.id);
            }
        }
    }

    #[test]
    fn an_added_effect_arrives_on() {
        for def in rows().iter().filter(|d| d.id.ends_with(".on")) {
            assert_eq!(def.default, 1.0, "{}", def.id);
        }
    }

    #[test]
    fn sanitise_drops_the_empty_the_duplicate_and_the_impossible() {
        let mut v = [0.0; ORDER_LEN];
        v[0] = 4.0;
        v[1] = 0.0;
        v[2] = 4.0;
        v[3] = 99.0;
        v[4] = f32::NAN;
        v[5] = 1.0;
        v[6] = -3.0;
        let o = Order::sanitise(v);
        assert_eq!(o.as_slice(), &[3, 0]);
    }

    #[test]
    fn add_takes_the_first_free_copy_and_stops_when_both_are_in() {
        let mut o = Order::EMPTY;
        let a = o.add(Kind::Chorus).unwrap();
        let b = o.add(Kind::Chorus).unwrap();
        assert_ne!(a, b);
        assert_eq!(kind_of(a), Kind::Chorus);
        assert_eq!(kind_of(b), Kind::Chorus);
        assert_eq!(o.add(Kind::Chorus), None);
        assert_eq!(o.len(), 2);
    }

    #[test]
    fn add_stops_when_the_chain_is_full() {
        let mut o = Order::EMPTY;
        let mut added = 0;
        for kind in Kind::ALL.into_iter().cycle().take(40) {
            if o.add(kind).is_some() {
                added += 1;
            }
        }
        assert_eq!(added, ORDER_LEN);
    }

    #[test]
    fn remove_closes_the_gap_and_move_swaps_neighbours() {
        let mut o = Order::EMPTY;
        let d = o.add(Kind::Delay).unwrap();
        let c = o.add(Kind::Chorus).unwrap();
        let r = o.add(Kind::Reverb).unwrap();
        assert_eq!(o.as_slice(), &[d as u8, c as u8, r as u8]);
        assert!(o.nudge(r, -1));
        assert_eq!(o.as_slice(), &[d as u8, r as u8, c as u8]);
        assert!(!o.nudge(d, -1), "stops at the front");
        assert!(!o.nudge(c, 1), "stops at the end");
        assert!(o.remove(r));
        assert_eq!(o.as_slice(), &[d as u8, c as u8]);
        assert!(!o.remove(r));
    }

    #[test]
    fn a_bank_holds_an_order_in_either_table() {
        let patch = ParamBank::new();
        let arr = ParamBank::for_table(crate::arrangement::params());
        for bank in [&patch, &arr] {
            assert!(order_of(bank).is_empty(), "a fresh chain is empty");
            let mut o = Order::EMPTY;
            o.add(Kind::Reverb);
            o.add(Kind::Drive);
            set_order(bank, &o);
            assert_eq!(order_of(bank), o);
        }
    }

    #[test]
    fn add_remove_and_move_work_on_either_bank() {
        let patch = ParamBank::new();
        let arr = ParamBank::for_table(crate::arrangement::params());
        for bank in [&patch, &arr] {
            let d = add_to_chain(bank, Kind::Delay).unwrap();
            let c = add_to_chain(bank, Kind::Chorus).unwrap();
            assert_eq!(order_of(bank).as_slice(), &[d as u8, c as u8]);
            assert_eq!(move_in_chain(bank, c, -1), Ok(true));
            assert_eq!(order_of(bank).as_slice(), &[c as u8, d as u8]);
            assert_eq!(move_in_chain(bank, c, -1), Ok(false));
            assert!(move_in_chain(bank, 99, 1).is_err());
            remove_from_chain(bank, c).unwrap();
            assert!(remove_from_chain(bank, c).is_err());
            assert_eq!(order_of(bank).as_slice(), &[d as u8]);
        }
    }

    #[test]
    fn a_removed_effect_keeps_its_settings_and_adding_it_back_switches_it_on() {
        let bank = ParamBank::new();
        let n = add_to_chain(&bank, Kind::Delay).unwrap();
        bank.set_by_id(&row_id(n, "delay.time"), 777.0);
        bank.set_by_id(&row_id(n, "delay.on"), 0.0);
        remove_from_chain(&bank, n).unwrap();
        assert_eq!(bank.get_by_id(&row_id(n, "delay.time")), Some(777.0));
        let again = add_to_chain(&bank, Kind::Delay).unwrap();
        assert_eq!(again, n, "the first free copy is the same one");
        assert_eq!(bank.get_by_id(&row_id(n, "delay.time")), Some(777.0));
        assert_eq!(bank.get_by_id(&row_id(n, "delay.on")), Some(1.0));
    }

    #[test]
    fn a_third_copy_is_refused_with_a_reason() {
        let bank = ParamBank::new();
        add_to_chain(&bank, Kind::Echo).unwrap();
        add_to_chain(&bank, Kind::Echo).unwrap();
        let err = add_to_chain(&bank, Kind::Echo).unwrap_err();
        assert!(err.contains("both copies"), "{err}");
    }

    #[test]
    fn values_round_trip() {
        let mut o = Order::EMPTY;
        o.add(Kind::Echo);
        o.add(Kind::Drive);
        assert_eq!(Order::sanitise(o.to_values()), o);
    }

    #[test]
    fn an_lfo_cannot_be_linked_to_the_order() {
        use crate::modulation::{LinkError, ModSet};
        let mut set = ModSet::empty();
        for p in 0..ORDER_LEN {
            assert!(matches!(
                set.link(&order_id(p), 1, 0.0, 1.0),
                Err(LinkError::Stepped(_))
            ));
        }
    }

    #[test]
    fn order_rows_are_recognised_in_both_tables() {
        assert!(is_order_row("fx.order.0"));
        assert!(is_order_row("arrangement.fx.order.15"));
        assert!(!is_order_row("fx.order.16"));
        assert!(!is_order_row("fx.3.chorus.mix"));
    }
}
