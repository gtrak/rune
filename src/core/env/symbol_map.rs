use crate::core::{
    gc::{Block, Context},
    object::{CloneIn, Function, LispBuffer, Symbol, WithLifetime},
};
use anyhow::Result;
use rune_core::hashmap::HashMap;

pub(crate) struct SymbolMap {
    map: SymbolMapCore,
    block: Block<true>,
}

struct SymbolMapCore {
    map: HashMap<&'static str, Symbol<'static>>,
}

impl SymbolMapCore {
    fn with_capacity(cap: usize) -> Self {
        Self {
            map: HashMap::with_capacity_and_hasher(cap, std::hash::BuildHasherDefault::default()),
        }
    }

    fn get(&self, name: &str) -> Option<Symbol> {
        self.map.get(name).map(|x| unsafe { x.with_lifetime() })
    }

    fn intern<'ob>(&mut self, name: &str, block: &Block<true>, cx: &'ob Context) -> Symbol<'ob> {
        match self.get(name) {
            Some(x) => cx.bind(x),
            None => {
                let name = name.to_owned();
                // Leak the memory so that it is static
                let static_name: &'static str = unsafe {
                    let name_ptr: *const str = Box::into_raw(name.into_boxed_str());
                    &*name_ptr
                };
                let sym = Symbol::new(static_name, block);
                self.map.insert(static_name, unsafe { sym.with_lifetime() });
                cx.bind(sym)
            }
        }
    }

    fn pre_init(&mut self, sym: Symbol<'static>) {
        use std::collections::hash_map::Entry;
        let name = sym.get().name();
        let entry = self.map.entry(name);
        assert!(matches!(entry, Entry::Vacant(_)), "Attempt to intitalize {name} twice");
        entry.or_insert_with(|| sym);
    }
}

impl SymbolMap {
    pub(crate) fn intern<'ob>(&mut self, name: &str, cx: &'ob Context) -> Symbol<'ob> {
        self.map.intern(name, &self.block, cx)
    }

    pub(crate) fn set_func(&self, symbol: Symbol, func: Function) -> Result<()> {
        let new_func = func.clone_in(&self.block);
        self.block.uninterned_symbol_map.clear();
        #[cfg(miri)]
        new_func.untag().set_as_miri_root();
        // SAFETY: The object is marked read-only, we have cloned in the map's
        // context, and it is const, so calling this function is safe.
        unsafe { symbol.set_func(new_func) }
    }

    pub(crate) fn global_block(&self) -> &Block<true> {
        &self.block
    }

    pub(crate) fn create_buffer(&self, name: &str) -> &LispBuffer {
        LispBuffer::create(name.to_owned(), &self.block)
    }

    pub(crate) fn get(&self, name: &str) -> Option<Symbol> {
        self.map.get(name)
    }
}

// This file includes all symbol definitions. Generated by build.rs
include!(concat!(env!("OUT_DIR"), "/sym.rs"));

/// Intern a new symbol based on `name`
pub(crate) fn intern<'ob>(name: &str, cx: &'ob Context) -> Symbol<'ob> {
    interned_symbols().lock().unwrap().intern(name, cx)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::core::gc::{Context, RootSet};
    use crate::core::object::FunctionType;
    use crate::core::{cons::Cons, env::Env, object::Object};
    use rune_core::macros::{list, root};
    use std::mem::size_of;

    #[test]
    fn size() {
        assert_eq!(size_of::<isize>(), size_of::<Symbol>());
        assert_eq!(size_of::<isize>(), size_of::<Function>());
    }

    #[test]
    fn init() {
        let roots = &RootSet::default();
        let cx = &Context::new(roots);
        intern("foo", cx);
    }

    #[test]
    fn symbol_func() {
        let roots = &RootSet::default();
        let cx = &Context::new(roots);
        sym::init_symbols();
        let sym = Symbol::new_uninterned("foo", cx);
        assert_eq!("foo", sym.name());
        assert!(sym.func(cx).is_none());
        let func1 = Cons::new1(1, cx);
        unsafe {
            sym.set_func(func1.into()).unwrap();
        }
        let cell1 = sym.func(cx).unwrap();
        let FunctionType::Cons(before) = cell1.untag() else {
            unreachable!("Type should be a lisp function")
        };
        assert_eq!(before.car(), 1);
        let func2 = Cons::new1(2, cx);
        unsafe {
            sym.set_func(func2.into()).unwrap();
        }
        let cell2 = sym.func(cx).unwrap();
        let FunctionType::Cons(after) = cell2.untag() else {
            unreachable!("Type should be a lisp function")
        };
        assert_eq!(after.car(), 2);
        assert_eq!(before.car(), 1);

        unsafe {
            sym.set_func(sym::NIL.into()).unwrap();
        }
        assert!(!sym.has_func());
    }

    #[test]
    fn test_mutability() {
        let roots = &RootSet::default();
        let cx = &Context::new(roots);
        let cons = list!(1, 2, 3; cx);
        assert_eq!(cons, list!(1, 2, 3; cx));
        // is mutable
        if let crate::core::object::ObjectType::Cons(cons) = cons.untag() {
            cons.set_car(4.into()).unwrap();
        } else {
            unreachable!();
        }
        assert_eq!(cons, list!(4, 2, 3; cx));
        let sym = intern("cons-test", cx);
        crate::data::fset(sym, cons).unwrap();
        // is not mutable
        if let FunctionType::Cons(cons) = sym.func(cx).unwrap().untag() {
            assert!(cons.set_car(5.into()).is_err());
            let obj: Object = cons.into();
            assert_eq!(obj, list!(4, 2, 3; cx));
        } else {
            unreachable!();
        }
    }

    #[test]
    fn test_init_variables() {
        let roots = &RootSet::default();
        let cx = &Context::new(roots);
        root!(env, new(Env), cx);
        init_variables(cx, env);
    }
}
