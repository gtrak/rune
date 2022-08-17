use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use super::super::{
    cons::Cons,
    env::Symbol,
    object::{GcObj, RawObj},
};
use super::{Block, Context, RootSet, Trace};
use crate::core::error::{Type, TypeError};
use crate::core::object::{Gc, IntoObject, Object, WithLifetime};
use crate::hashmap::{HashMap, HashSet};

pub(crate) trait IntoRoot {
    type Out;
    unsafe fn into_root(self) -> Self::Out;
}

impl<T, U> IntoRoot for Gc<T>
where
    Gc<T>: WithLifetime<'static, Out = U>,
{
    type Out = U;
    unsafe fn into_root(self) -> U {
        self.with_lifetime()
    }
}

impl IntoRoot for &Rt<GcObj<'_>> {
    type Out = GcObj<'static>;
    unsafe fn into_root(self) -> GcObj<'static> {
        self.inner.with_lifetime()
    }
}

impl IntoRoot for &Cons {
    type Out = &'static Cons;
    unsafe fn into_root(self) -> &'static Cons {
        self.with_lifetime()
    }
}

impl IntoRoot for Symbol {
    type Out = Symbol;
    unsafe fn into_root(self) -> Symbol {
        self
    }
}

impl<T, Tx> IntoRoot for Option<T>
where
    T: IntoRoot<Out = Tx>,
{
    type Out = Option<Tx>;
    unsafe fn into_root(self) -> Self::Out {
        self.map(|x| x.into_root())
    }
}

impl<T, U, Tx, Ux> IntoRoot for (T, U)
where
    T: IntoRoot<Out = Tx>,
    U: IntoRoot<Out = Ux>,
{
    type Out = (Tx, Ux);
    unsafe fn into_root(self) -> (Tx, Ux) {
        (self.0.into_root(), self.1.into_root())
    }
}

impl<T: IntoRoot<Out = U>, U> IntoRoot for Vec<T> {
    type Out = Vec<U>;
    unsafe fn into_root(self) -> Vec<U> {
        self.into_iter().map(|x| x.into_root()).collect()
    }
}

impl<T> Trace for Gc<T> {
    fn mark(&self, stack: &mut Vec<RawObj>) {
        self.as_obj().trace_mark(stack);
    }
}

impl Trace for &Cons {
    fn mark(&self, stack: &mut Vec<RawObj>) {
        Cons::mark(self, stack);
    }
}

impl<T, U> Trace for (T, U)
where
    T: Trace,
    U: Trace,
{
    fn mark(&self, stack: &mut Vec<RawObj>) {
        self.0.mark(stack);
        self.1.mark(stack);
    }
}

impl Trace for Symbol {
    fn mark(&self, _stack: &mut Vec<RawObj>) {
        // TODO: implement
    }
}

/// Represents a Rooted object T. The purpose of this type is we cannot have
/// mutable references to the inner data, because the garbage collector will
/// need to trace it. This type will only give us a mut [`Rt`] (rooted mutable
/// reference) when we are also holding a reference to the Context, meaning that
/// garbage collection cannot happen.
pub(crate) struct Root<'rt, 'a, T> {
    data: *mut T,
    root_set: &'rt RootSet,
    // This lifetime parameter ensures that functions like mem::swap cannot be
    // called in a way that would lead to memory unsafety
    safety: PhantomData<&'a ()>,
}

impl<T: Debug> Debug for Root<'_, '_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(unsafe { &*self.data }, f)
    }
}

impl<'rt, T> Root<'rt, '_, T> {
    pub(crate) unsafe fn new(root_set: &'rt RootSet) -> Self {
        Self {
            data: std::ptr::null_mut(),
            root_set,
            safety: PhantomData,
        }
    }

    pub(crate) fn as_mut<'a>(&'a mut self, _cx: &'a Context) -> &'a mut Rt<T> {
        unsafe { self.deref_mut_unchecked() }
    }

    pub(crate) unsafe fn deref_mut_unchecked(&mut self) -> &mut Rt<T> {
        assert!(
            !self.data.is_null(),
            "Attempt to mutably deref uninitialzed Root"
        );
        &mut *self.data.cast::<Rt<T>>()
    }
}

impl<T> Deref for Root<'_, '_, T> {
    type Target = Rt<T>;

    fn deref(&self) -> &Self::Target {
        assert!(!self.data.is_null(), "Attempt to deref uninitialzed Root");
        unsafe { &*self.data.cast::<Rt<T>>() }
    }
}

impl<T> AsRef<Rt<T>> for Root<'_, '_, T> {
    fn as_ref(&self) -> &Rt<T> {
        self
    }
}

impl<'rt, T: Trace + 'static> Root<'rt, '_, T> {
    pub(crate) unsafe fn init<'a>(root: &'a mut Self, data: &'a mut T) -> &'a mut Root<'rt, 'a, T> {
        assert!(root.data.is_null(), "Attempt to reinit Root");
        let dyn_ptr = data as &mut dyn Trace as *mut dyn Trace;
        root.data = dyn_ptr.cast::<T>();
        root.root_set.roots.borrow_mut().push(dyn_ptr);
        // We need the safety lifetime to match the borrow
        std::mem::transmute::<&mut Root<'rt, '_, T>, &mut Root<'rt, 'a, T>>(root)
    }
}

impl<T> Drop for Root<'_, '_, T> {
    fn drop(&mut self) {
        if self.data.is_null() {
            eprintln!("Error: Root was dropped while still not set");
        } else {
            self.root_set.roots.borrow_mut().pop();
        }
    }
}

#[macro_export]
macro_rules! root {
    ($ident:ident, $cx:ident) => {
        root!(
            $ident,
            unsafe { $crate::core::gc::IntoRoot::into_root($ident) },
            $cx
        );
    };
    ($ident:ident, move($value:expr), $cx:ident) => {
        root!(
            $ident,
            unsafe { $crate::core::gc::IntoRoot::into_root($value) },
            $cx
        );
    };
    ($ident:ident, $value:expr, $cx:ident) => {
        let mut rooted = $value;
        let mut root: $crate::core::gc::Root<_> =
            unsafe { $crate::core::gc::Root::new($cx.get_root_set()) };
        let $ident = unsafe { $crate::core::gc::Root::init(&mut root, &mut rooted) };
    };
}

/// A Rooted type. If a type is wrapped in Rt, it is known to be rooted and hold
/// items passed garbage collection. This type is never used as an owned type,
/// only a reference. This ensures that underlying data does not move. In order
/// to access the inner data, the [`Rt::bind`] method must be used.
#[repr(transparent)]
pub(crate) struct Rt<T: ?Sized> {
    inner: T,
}

impl<T: ?Sized + Debug> Debug for Rt<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.inner, f)
    }
}

impl PartialEq for Rt<GcObj<'_>> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T: PartialEq<U>, U> PartialEq<U> for Rt<T> {
    fn eq(&self, other: &U) -> bool {
        self.inner == *other
    }
}

impl Deref for Rt<Symbol> {
    type Target = Symbol;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Rt<T> {
    pub(crate) fn bind<'ob, U>(&self, _: &'ob Context) -> U
    where
        T: WithLifetime<'ob, Out = U> + Copy,
    {
        unsafe { self.inner.with_lifetime() }
    }

    pub(crate) unsafe fn bind_unchecked<'ob, U>(&'ob self) -> U
    where
        T: WithLifetime<'ob, Out = U> + Copy,
    {
        self.inner.with_lifetime()
    }

    pub(crate) fn bind_slice<'ob, U>(slice: &[Rt<T>], _: &'ob Context) -> &'ob [U]
    where
        T: WithLifetime<'ob, Out = U>,
    {
        unsafe { &*(slice as *const [Rt<T>] as *const [U]) }
    }
}

impl<T> Rt<Gc<T>> {
    pub(crate) fn try_as<U, E>(&self) -> Result<&Rt<Gc<U>>, E>
    where
        Gc<T>: TryInto<Gc<U>, Error = E> + Copy,
    {
        let _: Gc<U> = self.inner.try_into()?;
        // SAFETY: This is safe because all Gc types have the same representation
        unsafe { Ok(&*((self as *const Self).cast::<Rt<Gc<U>>>())) }
    }

    // TODO: see if this can be removed
    pub(crate) fn as_cons(&self) -> &Rt<Gc<&Cons>> {
        match self.inner.as_obj().get() {
            crate::core::object::Object::Cons(_) => unsafe {
                &*(self as *const Self).cast::<Rt<Gc<&Cons>>>()
            },
            x => panic!("attempt to convert type that was not cons: {x}"),
        }
    }

    pub(crate) fn set<U>(&mut self, item: U)
    where
        U: IntoRoot<Out = Gc<T>>,
    {
        unsafe {
            self.inner = item.into_root();
        }
    }
}

impl TryFrom<&Rt<GcObj<'_>>> for Symbol {
    type Error = anyhow::Error;

    fn try_from(value: &Rt<GcObj>) -> Result<Self, Self::Error> {
        match value.inner.get() {
            Object::Symbol(sym) => Ok(sym),
            x => Err(TypeError::new(Type::Symbol, x).into()),
        }
    }
}

impl From<&Rt<GcObj<'_>>> for Option<()> {
    fn from(value: &Rt<GcObj<'_>>) -> Self {
        value.inner.nil().then_some(())
    }
}

impl Rt<GcObj<'static>> {
    pub(crate) fn try_as_option<T, E>(&self) -> Result<Option<&Rt<Gc<T>>>, E>
    where
        GcObj<'static>: TryInto<Gc<T>, Error = E>,
    {
        if self.inner.nil() {
            Ok(None)
        } else {
            let _: Gc<T> = self.inner.try_into()?;
            unsafe { Ok(Some(&*((self as *const Self).cast::<Rt<Gc<T>>>()))) }
        }
    }
}

impl Rt<GcObj<'_>> {
    pub(crate) fn get<'ob>(&self, cx: &'ob Context) -> Object<'ob> {
        self.bind(cx).get()
    }
}

impl<'ob> IntoObject<'ob> for &Rt<GcObj<'static>> {
    type Out = Object<'ob>;

    fn into_obj<const C: bool>(self, _block: &'ob Block<C>) -> Gc<Self::Out> {
        unsafe { self.inner.with_lifetime() }
    }
}

impl<'ob> IntoObject<'ob> for &Root<'_, '_, GcObj<'static>> {
    type Out = Object<'ob>;

    fn into_obj<const C: bool>(self, _block: &'ob Block<C>) -> Gc<Self::Out> {
        unsafe { self.inner.with_lifetime() }
    }
}

impl<'ob> IntoObject<'ob> for &mut Root<'_, '_, GcObj<'static>> {
    type Out = Object<'ob>;

    fn into_obj<const C: bool>(self, _block: &'ob Block<C>) -> Gc<Self::Out> {
        unsafe { self.inner.with_lifetime() }
    }
}

impl Rt<&Cons> {
    pub(crate) fn set(&mut self, item: &Cons) {
        self.inner = unsafe { std::mem::transmute(item) }
    }

    pub(crate) fn car(&self) -> &Rt<GcObj> {
        unsafe { &*self.inner.addr_car().cast::<Rt<GcObj>>() }
    }

    pub(crate) fn cdr(&self) -> &Rt<GcObj> {
        unsafe { &*self.inner.addr_cdr().cast::<Rt<GcObj>>() }
    }
}

impl<T, U> Deref for Rt<(T, U)> {
    type Target = (Rt<T>, Rt<U>);

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self as *const Self).cast::<(Rt<T>, Rt<U>)>() }
    }
}

impl<T, U> DerefMut for Rt<(T, U)> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(self as *mut Rt<(T, U)>).cast::<(Rt<T>, Rt<U>)>() }
    }
}

impl<T> Deref for Rt<Option<T>> {
    type Target = Option<Rt<T>>;
    fn deref(&self) -> &Self::Target {
        unsafe { &*(self as *const Self).cast::<Self::Target>() }
    }
}

impl<T> DerefMut for Rt<Option<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *(self as *mut Self).cast::<Self::Target>() }
    }
}

impl Rt<Option<GcObj<'static>>> {
    pub(crate) fn set(&mut self, obj: GcObj) {
        unsafe {
            self.inner = Some(obj.with_lifetime());
        }
    }
}

impl<T> Rt<Vec<T>> {
    pub(crate) fn push<U: IntoRoot<Out = T>>(&mut self, item: U) {
        self.inner.push(unsafe { item.into_root() });
    }
}

impl<T> Deref for Rt<Vec<T>> {
    type Target = Vec<Rt<T>>;
    fn deref(&self) -> &Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &*(self as *const Self).cast::<Self::Target>() }
    }
}

impl<T> DerefMut for Rt<Vec<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &mut *(self as *mut Self).cast::<Self::Target>() }
    }
}

impl<K, V> Rt<HashMap<K, V>>
where
    K: Eq + std::hash::Hash,
{
    pub(crate) fn insert<R: IntoRoot<Out = V>>(&mut self, k: K, v: R) {
        self.inner.insert(k, unsafe { v.into_root() });
    }
}

impl<K, V> Deref for Rt<HashMap<K, V>> {
    type Target = HashMap<K, Rt<V>>;
    fn deref(&self) -> &Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &*(self as *const Self).cast::<Self::Target>() }
    }
}

impl<K, V> DerefMut for Rt<HashMap<K, V>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &mut *(self as *mut Self).cast::<Self::Target>() }
    }
}

#[allow(dead_code)]
impl<T> Rt<HashSet<T>>
where
    T: Eq + std::hash::Hash,
{
    pub(crate) fn insert<R: IntoRoot<Out = T>>(&mut self, value: R) -> bool {
        self.inner.insert(unsafe { value.into_root() })
    }
}

impl<T> Deref for Rt<HashSet<T>> {
    type Target = HashSet<Rt<T>>;
    fn deref(&self) -> &Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &*(self as *const Self).cast::<Self::Target>() }
    }
}

impl<T> DerefMut for Rt<HashSet<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `Rt<T>` has the same memory layout as `T`.
        unsafe { &mut *(self as *mut Self).cast::<Self::Target>() }
    }
}

#[cfg(test)]
mod test {
    use crate::core::object::nil;

    use super::super::RootSet;
    use super::*;

    #[test]
    fn indexing() {
        let root = &RootSet::default();
        let cx = &Context::new(root);
        let mut vec: Rt<Vec<GcObj<'static>>> = Rt { inner: vec![] };

        vec.push(nil());
        assert_eq!(vec[0], nil());
        let str1 = cx.add("str1");
        let str2 = cx.add("str2");
        vec.push(str1);
        vec.push(str2);
        let slice = &vec[0..3];
        assert_eq!(vec![nil(), str1, str2], Rt::bind_slice(slice, cx));
    }
}
