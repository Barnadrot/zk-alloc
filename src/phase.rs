use crate::arena;
use crate::pressure;

/// Signal that a proving phase has ended. All thread-local arenas
/// reset their bump regions and compact their size-class pools.
///
/// Call this between proving phases (e.g., after witness generation
/// completes, after commitment, after sumcheck). Each call is O(1)
/// per arena — no iteration over freed objects.
///
/// Safe to call from any thread. Only resets the calling thread's arena.
/// For full reset across all Rayon workers, call from within a
/// `rayon::broadcast` or at a join point.
pub fn phase_boundary() {
    pressure::update_policy();
    arena::reset_arena();
}
