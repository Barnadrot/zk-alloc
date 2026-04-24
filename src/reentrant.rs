use std::sync::atomic::{AtomicI32, Ordering};

static KEY: AtomicI32 = AtomicI32::new(-1);

fn get_key() -> libc::pthread_key_t {
    let k = KEY.load(Ordering::Relaxed);
    if k >= 0 {
        return k as libc::pthread_key_t;
    }
    let mut key: libc::pthread_key_t = 0;
    unsafe { libc::pthread_key_create(&mut key, None) };
    KEY.store(key as i32, Ordering::Relaxed);
    key
}

#[inline]
pub fn is_reentrant() -> bool {
    let key = get_key();
    unsafe { libc::pthread_getspecific(key) as usize != 0 }
}

#[inline]
pub fn set_reentrant(val: bool) {
    let key = get_key();
    unsafe { libc::pthread_setspecific(key, val as usize as *mut libc::c_void) };
}
