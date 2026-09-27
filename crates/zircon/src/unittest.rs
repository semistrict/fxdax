//! Stand-ins for the kernel's `unittest` crate, so that Zircon's Rust unit
//! tests run under the host test harness unchanged.

// Not every test uses every macro.
#![allow(unused_macros, unused_imports)]

macro_rules! expect_true {
    ($cond:expr $(, $($msg:tt)+)?) => { assert!($cond $(, $($msg)+)?) };
}
macro_rules! expect_false {
    ($cond:expr $(, $($msg:tt)+)?) => { assert!(!$cond $(, $($msg)+)?) };
}
macro_rules! expect_eq {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert_eq!($a, $b $(, $($msg)+)?) };
}
macro_rules! expect_ne {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert_ne!($a, $b $(, $($msg)+)?) };
}
macro_rules! expect_gt {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert!($a > $b $(, $($msg)+)?) };
}
macro_rules! expect_ge {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert!($a >= $b $(, $($msg)+)?) };
}
macro_rules! expect_lt {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert!($a < $b $(, $($msg)+)?) };
}
macro_rules! expect_le {
    ($a:expr, $b:expr $(, $($msg:tt)+)?) => { assert!($a <= $b $(, $($msg)+)?) };
}
macro_rules! expect_ok {
    ($res:expr $(, $($msg:tt)+)?) => { assert!(($res).is_ok() $(, $($msg)+)?) };
}
macro_rules! assert_ok {
    ($res:expr $(, $($msg:tt)+)?) => { assert!(($res).is_ok() $(, $($msg)+)?) };
}
macro_rules! unwrap_ok {
    ($res:expr $(, $($msg:tt)+)?) => { ($res).expect(concat!("", $($($msg)+)?)) };
}
pub(crate) use {
    assert_ok, expect_eq, expect_false, expect_ge, expect_gt, expect_le, expect_lt, expect_ne,
    expect_ok, expect_true, unwrap_ok,
};
