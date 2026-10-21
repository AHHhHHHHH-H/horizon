//! The Barter leg: gold from a bastion's gold blocks, thrown to piglins one
//! ingot at a time for fire resistance (and the pearls that come with it),
//! then the potion drunk on the way to the blaze spawner, the way 1.16+
//! speedrunners stop out-reflexing blazes and become immune to them.
//!
//! `gameplay/piglin_bartering` (469 weight): a fire resistance potion 8, a
//! splash one 8, pearls 10 (2–4). About 29 ingots a fire resistance potion
//! of either kind (59 for the drinkable one alone) and 0.064 pearls an
//! ingot. A bastion's walls and centre hold a few gold blocks (9 ingots
//! each); nether gold ore gives nuggets, which take a crafting table, so
//! it mines gold blocks only.
use super::{Game, NetherRun, at_block, count};
use crate::game::{
    Dim,
    companion::Body,
    companion_body::{Steer, mob_at},
    companion_work::{eye, feet},
    lan::LanHost,
};
use horizon_client::{
    companion::{self as c, CompanionCommand, Want, moves::Goal},
    items::ItemStack,
    mobs::Who,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use serde_json::{Value, json};

const GOLD_INGOT: &str = "minecraft:gold_ingot";
const GOLD_BLOCK: &str = "minecraft:gold_block";
const POTION: &str = "minecraft:potion";
const SPLASH: &str = "minecraft:splash_potion";
const PEARL: &str = "minecraft:ender_pearl";
/// A bastion further than this from where it stands is not worth the way.
const BASTION_REACH: f64 = 200.0;
/// Gold blocks it mines at a time, and at most.
const BLOCKS_EACH: u32 = 3;
const BLOCKS_MOST: u32 = 9;
/// Ticks between two ingots thrown.
const TOSS_EVERY: u64 = 8;
/// How long it waits for piglins that don't come before it gives up.
const NO_PIGLINS: u64 = 900;

/// What the leg did, for the status and the harness.
#[derive(Debug, Default)]
pub(super) struct Barter {
    pub started: Option<u64>,
    pub ended: Option<u64>,
    /// Why it was skipped or ended.
    pub outcome: Option<String>,
    pub mined: u32,
    pub thrown: u32,
    pub pearls_at_start: u32,
    /// The most fire resistance potions it carried (drinkable, splash).
    pub potions: u32,
    pub splash: u32,
    pub drunk: u32,
    collects: u32,
    last_toss: u64,
    /// Since when no piglin was about to trade with.
    alone_since: Option<u64>,
    /// The ingots it carried last tick (thrown ones counted once).
    ingots: u32,
}

impl Barter {
    pub fn status(&self, now: u64, pearls: u32) -> Value {
        json!({
            "outcome": self.outcome,
            "ticks": self.started.map(|s| self.ended.unwrap_or(now).saturating_sub(s)),
            "gold_blocks": self.mined,
            "ingots_thrown": self.thrown,
            // What the piglins kept (thrown ones it picked up again and
            // threw twice count once).
            "ingots_used": (self.mined * 9).saturating_sub(self.ingots),
            "fire_res_potions": self.potions,
            "fire_res_splash": self.splash,
            "drunk": self.drunk,
            "pearls": pearls.saturating_sub(self.pearls_at_start),
        })
    }
}

/// A fire resistance potion (drinkable or splash).
fn fire_res(s: &ItemStack) -> bool {
    (&*s.item == POTION || &*s.item == SPLASH)
        && s.potion().is_some_and(|p| p.ends_with("fire_resistance"))
}

fn fire_res_count(inv: &[Option<ItemStack>], item: &str) -> u32 {
    inv.iter()
        .flatten()
        .filter(|s| &*s.item == item && fire_res(s))
        .map(|s| s.count)
        .sum()
}

impl Game<'_> {
    pub(super) fn leg_barter(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if b.dim != Dim::Nether {
            return self.back_to_nether(host, b, run, pos);
        }
        let Some(g) = host.hub.guest(b.conn) else {
            return;
        };
        let (ingots, blocks, pearls) = (
            count(&g.inventory, GOLD_INGOT),
            count(&g.inventory, GOLD_BLOCK),
            count(&g.inventory, PEARL),
        );
        let (potions, splash) = (
            fire_res_count(&g.inventory, POTION),
            fire_res_count(&g.inventory, SPLASH),
        );
        if run.barter.started.is_none() {
            run.barter.started = Some(now);
            run.barter.pearls_at_start = pearls;
            run.barter.ingots = ingots;
        }
        run.barter.potions = run.barter.potions.max(potions);
        run.barter.splash = run.barter.splash.max(splash);
        if ingots < run.barter.ingots {
            run.barter.thrown += run.barter.ingots - ingots;
        }
        run.barter.ingots = ingots;
        // Where: a bastion it knows of near the way (located at Scout and
        // All-knowing; the others go on to the fortress). Gold it carries
        // already goes to any piglins about.
        let carrying = ingots + blocks > 0;
        if !b.senses.level.knows_structures() && !carrying {
            return self.barter_end(b, run, now, "no bastion known");
        }
        if run.bastion.is_none() && b.senses.level.knows_structures() {
            run.bastion = self.companion_structure(b, "bastion_remnant", pos);
            if run.fortress.is_none() {
                run.fortress = self.companion_structure(b, "fortress", pos);
            }
            if let Some(bastion) = run.bastion {
                log::info!(
                    "[companion] {} nether: barter: bastion at {bastion:?}, {:.0} blocks; fortress at {:?}, {:.0} from the bastion",
                    b.file.name,
                    c::horizontal(pos, at_block(bastion)),
                    run.fortress,
                    run.fortress
                        .map_or(0.0, |f| c::horizontal(at_block(f), at_block(bastion)))
                );
            }
        }
        // Too far, it keeps to the fortress (a run-ending walk both ways for
        // a potion that comes one barter in 29).
        let bastion = run.bastion.filter(|p| {
            c::horizontal(pos, at_block(*p)) <= BASTION_REACH || run.barter.collects > 0
        });
        if bastion.is_none() && !carrying && run.barter.mined == 0 {
            let why = match run.bastion {
                Some(p) => format!(
                    "the bastion is {:.0} blocks off",
                    c::horizontal(pos, at_block(p))
                ),
                None => "no bastion near".into(),
            };
            return self.barter_end(b, run, now, &why);
        }
        let far = bastion.map_or(0.0, |p| c::horizontal(pos, at_block(p)));
        // Mining and crafting run as the body's work.
        if !b.work.tasks.is_empty() {
            run.note = if blocks > 0 && ingots == 0 {
                "crafting gold ingots".into()
            } else {
                format!("mining gold blocks, {far:.0} blocks from the bastion")
            };
            return;
        }
        // Its loot on the floor first (pearls, potions): picked up.
        if let Some(at) = self
            .barter_loot_near(pos, 10.0)
            .filter(|_| run.barter.thrown > 0)
        {
            run.note = "picking up the barter".into();
            let _ = self.nether_go(host, b, run, Goal::near(feet(at), 0.4), pos);
            return;
        }
        if blocks > 0 {
            run.barter.mined += blocks;
            run.note = "crafting gold ingots".into();
            self.companion_start_work(
                host,
                b,
                CompanionCommand::Craft {
                    item: GOLD_INGOT.into(),
                    count: blocks * 9,
                    gather: false,
                },
            );
            return;
        }
        let pending = self.barter_pending(b, pos);
        if ingots == 0 {
            if potions + splash > 0
                || run.barter.mined >= BLOCKS_MOST
                || run.barter.collects >= 4
                || bastion.is_none()
            {
                // The last of the trades still to come down.
                if pending && now.saturating_sub(run.barter.last_toss) < 200 {
                    run.note = "waiting for the last trades".into();
                    b.keys = Default::default();
                    return;
                }
                let why = if potions + splash > 0 {
                    "fire resistance"
                } else {
                    "out of gold, no fire resistance"
                };
                return self.barter_end(b, run, now, why);
            }
            let Some(bastion) = bastion else {
                return;
            };
            if far > 40.0 {
                run.note = format!("to the bastion, {far:.0} blocks");
                if self.detour(host, b, run, pos) {
                    return;
                }
                if self.nether_go(host, b, run, Goal::column(bastion, 32.0), pos) == Steer::Stuck {
                    run.stuck += 1;
                    if run.stuck >= 3 {
                        return self.barter_end(b, run, now, "no way to the bastion");
                    }
                    run.waypoint = Some(horizon_client::companion::nether::detour(
                        feet(pos),
                        [bastion[0], bastion[2]],
                        run.stuck,
                        24,
                    ));
                }
                return;
            }
            run.barter.collects += 1;
            run.note = "mining gold blocks".into();
            log::info!(
                "[companion] {} nether: barter: mining gold blocks ({} so far)",
                b.file.name,
                run.barter.mined
            );
            self.companion_start_work(
                host,
                b,
                CompanionCommand::Collect {
                    what: Want::Block(GOLD_BLOCK.into()),
                    count: BLOCKS_EACH,
                },
            );
            return;
        }
        // Ingots in hand: one at a time to each calm piglin about, as many on
        // the floor as there are piglins to take them.
        let from = eye(pos);
        let calm: Vec<(u32, [f64; 3], f64)> = self
            .mobs
            .iter()
            .filter(|m| {
                m.alive()
                    && &*m.kind.id == "minecraft:piglin"
                    && !m.is_baby()
                    && m.extra.off_hand.is_none()
                    && !(m.ai.target == Some(Who::Player)
                        && host.mob_players.get(&m.id) == Some(&b.conn))
            })
            .map(|m| (m.id, mob_at(m), c::distance(mob_at(m), pos)))
            .filter(|(_, at, d)| *d < 24.0 && self.in_sight(from, [at[0], at[1] + 1.0, at[2]]))
            .collect();
        let on_floor = self
            .dropped
            .iter()
            .filter(|d| &*d.stack.item == GOLD_INGOT)
            .filter(|d| c::distance([d.position.x, d.position.y, d.position.z], pos) < 12.0)
            .count();
        let Some(&(_, at, d)) = calm.iter().min_by(|x, y| x.2.total_cmp(&y.2)) else {
            if pending {
                run.note = "waiting for the piglins".into();
                b.keys = Default::default();
                run.barter.alone_since = None;
                return;
            }
            let since = *run.barter.alone_since.get_or_insert(now);
            if now.saturating_sub(since) > NO_PIGLINS {
                return self.barter_end(b, run, now, "no piglins to trade with");
            }
            // Piglins live in the bastion: toward its middle.
            run.note = "looking for piglins".into();
            if let Some(bastion) = bastion {
                let _ = self.nether_go(host, b, run, Goal::column(bastion, 8.0), pos);
            }
            return;
        };
        run.barter.alone_since = None;
        if d > 4.5 {
            run.note = format!("to a piglin, {d:.0} blocks");
            let _ = self.nether_go(host, b, run, Goal::near(feet(at), 3.0), pos);
            return;
        }
        b.keys = Default::default();
        run.note = format!("bartering, {} ingots thrown", run.barter.thrown);
        if on_floor < calm.len() && now.saturating_sub(run.barter.last_toss) >= TOSS_EVERY {
            run.barter.last_toss = now;
            self.companion_toss(host, b, GOLD_INGOT, [at[0], at[1] + 0.2, at[2]], pos);
        }
    }

    pub(super) fn barter_end(&mut self, b: &mut Body, run: &mut NetherRun, now: u64, why: &str) {
        log::info!(
            "[companion] {} nether: barter: {why} ({} gold blocks, {} ingots thrown, {} + {} fire resistance)",
            b.file.name,
            run.barter.mined,
            run.barter.thrown,
            run.barter.potions,
            run.barter.splash
        );
        run.barter.outcome = Some(why.into());
        run.barter.started.get_or_insert(now);
        run.barter.ended = Some(now);
        self.companion_nether_next(b, run, now);
    }

    /// A piglin admiring gold, or an ingot on the floor near one: trades to
    /// come.
    fn barter_pending(&self, b: &Body, pos: [f64; 3]) -> bool {
        let _ = b;
        self.mobs.iter().any(|m| {
            m.alive()
                && &*m.kind.id == "minecraft:piglin"
                && m.extra.off_hand.as_deref() == Some(GOLD_INGOT)
                && c::distance(mob_at(m), pos) < 16.0
        }) || self.dropped.iter().any(|d| {
            &*d.stack.item == GOLD_INGOT
                && c::distance([d.position.x, d.position.y, d.position.z], pos) < 12.0
        })
    }

    /// The nearest pearl or fire resistance potion on the floor within
    /// `radius`.
    fn barter_loot_near(&self, pos: [f64; 3], radius: f64) -> Option<[f64; 3]> {
        self.dropped
            .iter()
            .filter(|d| &*d.stack.item == PEARL || fire_res(&d.stack))
            .filter(|d| d.pickup_delay < 20)
            .map(|d| [d.position.x, d.position.y, d.position.z])
            .filter(|at| c::distance(*at, pos) <= radius && (at[1] - pos[1]).abs() < 4.0)
            .min_by(|x, y| c::distance(*x, pos).total_cmp(&c::distance(*y, pos)))
    }

    /// One of `item` thrown toward `to` (`Player.drop`: it flies the way it
    /// looks).
    fn companion_toss(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        item: &str,
        to: [f64; 3],
        pos: [f64; 3],
    ) {
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return;
        };
        let Some(i) = host.hub.guests[gi]
            .inventory
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| &*s.item == item))
        else {
            return;
        };
        b.yaw = c::yaw_toward(pos, to) as f32;
        b.pitch = crate::game::companion_body::look_pitch(eye(pos), to);
        self.companion_hold_slot(host, b, i);
        let g = &mut host.hub.guests[gi];
        (g.yaw, g.pitch) = (b.yaw, b.pitch);
        self.lan_guest_packet(
            host,
            b.conn,
            S::PlayerAction(play::PlayerAction {
                action: 5,
                pos: [0; 3],
                direction: 0,
                sequence: 0,
            }),
        );
    }

    /// Fire resistance before the blazes: drunk (or splashed at its feet)
    /// once the spawner is close or a blaze is about, while none is on it.
    /// True when it took the tick.
    pub(super) fn companion_fire_res(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) -> bool {
        if run.leg != horizon_client::companion::nether::Leg::Blazes || b.eating.is_some() {
            return false;
        }
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return false;
        };
        let g = &host.hub.guests[gi];
        // On already, for more than the walk to the spawner.
        if g.effects
            .get("fire_resistance")
            .is_some_and(|e| e.duration < 0 || e.duration > 200)
        {
            return false;
        }
        let near_spawner = run
            .spawner
            .is_some_and(|s| c::distance(at_block(s), pos) < 32.0);
        let blaze_near = self.mobs.iter().any(|m| {
            m.alive() && &*m.kind.id == "minecraft:blaze" && c::distance(mob_at(m), pos) < 24.0
        });
        if !near_spawner && !blaze_near {
            return false;
        }
        let pick = |item: &str| {
            g.inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| &*s.item == item && fire_res(s)))
        };
        // The splash first (one tick, at its feet), else drink.
        if let Some(i) = pick(SPLASH) {
            b.pitch = 90.0;
            self.companion_hold_slot(host, b, i);
            let g = &mut host.hub.guests[gi];
            (g.yaw, g.pitch) = (b.yaw, b.pitch);
            let yaw = b.yaw;
            self.lan_guest_packet(
                host,
                b.conn,
                S::UseItem(play::UseItem {
                    hand: 0,
                    sequence: 0,
                    yaw,
                    pitch: 90.0,
                }),
            );
            run.barter.drunk += 1;
            run.note = "splashing fire resistance".into();
            log::info!(
                "[companion] {} nether: fire resistance splashed",
                b.file.name
            );
            return true;
        }
        let Some(i) = pick(POTION) else {
            return false;
        };
        self.companion_hold_slot(host, b, i);
        let g = &host.hub.guests[gi];
        let (yaw, pitch) = (g.yaw, g.pitch);
        let before = count(&g.inventory, POTION);
        self.lan_guest_packet(
            host,
            b.conn,
            S::UseItem(play::UseItem {
                hand: 0,
                sequence: 0,
                yaw,
                pitch,
            }),
        );
        let started = host
            .play
            .get(&b.conn)
            .and_then(crate::game::lan_actions::GuestPlay::using)
            .is_some();
        if started {
            b.eating = Some((before, POTION.into()));
            run.barter.drunk += 1;
            run.note = "drinking fire resistance".into();
            log::info!(
                "[companion] {} nether: drinking fire resistance",
                b.file.name
            );
        }
        started
    }
}
