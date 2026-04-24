use crate::arena;
use crate::pressure;

pub fn phase_boundary() {
    pressure::update_policy();
    arena::compact_pools();
}
