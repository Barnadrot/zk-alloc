use crate::arena;
use crate::pressure;

pub fn phase_boundary() {
    pressure::update_policy();
    arena::compact_pools();
    crate::activate_arena_impl();
}

pub fn deactivate_arena() {
    crate::deactivate_arena_impl();
    arena::deactivate();
}
