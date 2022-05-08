use crate::object::GcObj;
use fn_macros::defun;

#[defun]
fn string_match<'ob>(_regexp: &str, _string: &str, _start: Option<i64>) -> GcObj<'ob> {
    // TODO: implement
    GcObj::NIL
}

#[defun]
fn string_equal(s1: &str, s2: &str) -> bool {
    s1 == s2
}

defsubr!(string_match, string_equal);
