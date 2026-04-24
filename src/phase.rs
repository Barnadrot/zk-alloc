use crate::arena;
use crate::pressure;

pub fn phase_boundary() {
    arena::reclaim_dead_slabs();
    pressure::update_policy();
    arena::compact_pools();
}
