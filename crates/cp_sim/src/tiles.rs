//! Tile purchase (T-150, design §16 M2): `BuyTile` takes a tile that touches an owned one, at
//! the price of its ring, while the number of owned tiles stays below `1 +` the `tiles` of the
//! milestones reached.

use std::sync::Arc;

use bevy_ecs::world::World;
use cp_content::{ContentDb, SimContent};
use cp_core::treasury::{charge_funds, check_funds};
use cp_core::{CommandError, Cost};
use cp_terrain::{SimTerrain, TILES_PER_SIDE, TerrainCommand, TileOwnership, ring};

use crate::Progress;

fn rejected(reason: &str) -> CommandError {
    CommandError::Rejected {
        domain: "terrain",
        reason: reason.to_owned(),
    }
}

/// How many tiles the city may own at `progress`: the start tile and the `tiles` of every
/// milestone reached. Without milestones in the content there is no limit.
pub fn tile_limit(content: Option<&ContentDb>, progress: Progress) -> usize {
    let list = content.map_or(&[][..], |c| c.milestones());
    if list.is_empty() {
        return usize::MAX;
    }
    let upto = usize::from(progress.reached).min(list.len() - 1);
    1 + list[..=upto]
        .iter()
        .map(|m| usize::from(m.tiles))
        .sum::<usize>()
}

/// The price of tile `tile`, by its ring around `start`.
pub fn tile_price(content: Option<&ContentDb>, start: (u8, u8), tile: (u8, u8)) -> Cost {
    let prices = content.map_or_else(
        || cp_content::Balance::default().tile_price,
        |c| c.balance().tile_price,
    );
    let r = usize::from(ring(start, tile).clamp(1, prices.len() as u8));
    Cost::from_units(prices[r - 1])
}

/// What a tile of the map is to the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    Owned,
    /// Touches an owned tile and the milestone allows one more.
    Buyable(Cost),
    /// Touches an owned tile, but the milestone limit is reached.
    NeedsMilestone,
    /// Does not touch an owned tile.
    Far,
}

/// The state of `tile` in `view`.
pub fn tile_state(view: &crate::SimView, tile: (u8, u8)) -> TileState {
    let (tx, tz) = (usize::from(tile.0), usize::from(tile.1));
    if view.tiles.is_open(tx, tz) {
        return TileState::Owned;
    }
    if !view.tiles.touches_owned(tx, tz) {
        return TileState::Far;
    }
    let content = view.content.as_deref();
    if view.tiles.open_count() >= tile_limit(content, view.progress) {
        return TileState::NeedsMilestone;
    }
    let start = view
        .terrain
        .as_ref()
        .map_or((2, 2), |t| t.meta().start_tile);
    TileState::Buyable(tile_price(content, start, tile))
}

pub fn apply(world: &mut World, cmd: &TerrainCommand) -> Result<Cost, CommandError> {
    let TerrainCommand::BuyTile { tile } = *cmd;
    let (tx, tz) = (usize::from(tile.0), usize::from(tile.1));
    if tx >= TILES_PER_SIDE || tz >= TILES_PER_SIDE {
        return Err(rejected("tile is off the map"));
    }
    let owned = *world.resource::<TileOwnership>();
    if owned.is_open(tx, tz) {
        return Err(rejected("tile is already owned"));
    }
    if !owned.touches_owned(tx, tz) {
        return Err(rejected("tile does not touch an owned tile"));
    }
    let content = world.get_resource::<SimContent>().map(|c| Arc::clone(&c.0));
    let progress = world
        .get_resource::<Progress>()
        .copied()
        .unwrap_or_default();
    if owned.open_count() >= tile_limit(content.as_deref(), progress) {
        return Err(rejected("no more tiles until the next milestone"));
    }
    let start = world
        .get_resource::<SimTerrain>()
        .map_or((2, 2), |t| t.0.meta().start_tile);
    let price = tile_price(content.as_deref(), start, tile);
    check_funds(world, price)?;
    world.resource_mut::<TileOwnership>().set(tx, tz, true);
    charge_funds(world, price);
    Ok(price)
}
