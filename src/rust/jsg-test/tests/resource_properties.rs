// Copyright (c) 2026 Cloudflare, Inc.
// Licensed under the Apache 2.0 license found in the LICENSE file or at:
//     https://opensource.org/licenses/Apache-2.0

//! Tests for `#[jsg_prototype_property]`, `#[jsg_instance_property]`, and
//! `#[jsg_inspect_property]`.

use std::cell::Cell;
use std::cell::RefCell;

use jsg::Number;
use jsg::ToJS;
use jsg_macros::jsg_inspect_property;
use jsg_macros::jsg_instance_property;
use jsg_macros::jsg_method;
use jsg_macros::jsg_prototype_property;
use jsg_macros::jsg_resource;
use jsg_macros::jsg_static_constant;

// =============================================================================
// Shared test resources
// =============================================================================

/// A counter with a read/write value, a read-only label, and an inspect property.
#[jsg_resource]
struct Counter {
    value: Cell<f64>,
    label: RefCell<String>,
}

#[jsg_resource]
impl Counter {
    // Read/write prototype property — detected from get_/set_ prefix.
    #[jsg_prototype_property]
    pub fn get_value(&self) -> Number {
        Number::new(self.value.get())
    }

    #[jsg_prototype_property]
    pub fn set_value(&self, v: Number) {
        self.value.set(v.value());
    }

    // Read-only prototype property (no matching set_).
    #[jsg_prototype_property]
    pub fn get_label(&self) -> String {
        self.label.borrow().clone()
    }

    // A regular method coexisting with properties.
    #[jsg_method]
    pub fn reset(&self) {
        self.value.set(0.0);
    }

    // Inspect property — hidden from normal enumeration.
    #[jsg_inspect_property]
    pub fn debug_info(&self) -> String {
        format!(
            "Counter(value={}, label={})",
            self.value.get(),
            self.label.borrow()
        )
    }

    // Static constant coexisting with properties.
    #[jsg_static_constant]
    pub const MAX_VALUE: f64 = 1_000_000.0;
}

impl Counter {
    fn new(value: f64, label: impl Into<String>) -> Self {
        Self {
            value: Cell::new(value),
            label: RefCell::new(label.into()),
        }
    }
}

/// A token with a read/write instance property and a read-only one.
#[jsg_resource]
struct Token {
    id: RefCell<String>,
    kind: String,
}

#[jsg_resource]
impl Token {
    #[jsg_instance_property]
    pub fn get_id(&self) -> String {
        self.id.borrow().clone()
    }

    #[jsg_instance_property]
    pub fn set_id(&self, v: String) {
        *self.id.borrow_mut() = v;
    }

    // Read-only instance property.
    #[jsg_instance_property]
    pub fn get_kind(&self) -> String {
        self.kind.clone()
    }
}

impl Token {
    fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: RefCell::new(id.into()),
            kind: kind.into(),
        }
    }
}

/// A resource with multi-word `snake_case` property names to test camelCase conversion.
#[jsg_resource]
struct MultiWord {
    first_name: RefCell<String>,
    last_name: RefCell<String>,
}

#[jsg_resource]
impl MultiWord {
    #[jsg_prototype_property]
    pub fn get_first_name(&self) -> String {
        self.first_name.borrow().clone()
    }

    #[jsg_prototype_property]
    pub fn set_first_name(&self, v: String) {
        *self.first_name.borrow_mut() = v;
    }

    #[jsg_prototype_property]
    pub fn get_last_name(&self) -> String {
        self.last_name.borrow().clone()
    }
}

impl MultiWord {
    fn new(first: impl Into<String>, last: impl Into<String>) -> Self {
        Self {
            first_name: RefCell::new(first.into()),
            last_name: RefCell::new(last.into()),
        }
    }
}

/// A resource with an explicit name override.
#[jsg_resource]
struct ExplicitName {
    x: Cell<f64>,
}

#[jsg_resource]
impl ExplicitName {
    #[jsg_prototype_property(name = "myValue")]
    pub fn get_something(&self) -> Number {
        Number::new(self.x.get())
    }

    #[jsg_prototype_property(name = "myValue")]
    pub fn set_something(&self, v: Number) {
        self.x.set(v.value());
    }

    #[jsg_inspect_property(name = "debugX")]
    pub fn internal_debug(&self) -> Number {
        Number::new(self.x.get())
    }
}

impl ExplicitName {
    fn new(x: f64) -> Self {
        Self { x: Cell::new(x) }
    }
}

/// A resource where a property returns `Option<T>` (can be null in JS).
#[jsg_resource]
struct MaybeHolder {
    inner: Cell<Option<f64>>,
}

#[jsg_resource]
impl MaybeHolder {
    #[jsg_prototype_property]
    pub fn get_value(&self) -> Option<Number> {
        self.inner.get().map(Number::new)
    }

    #[jsg_prototype_property]
    pub fn set_value(&self, v: Number) {
        self.inner.set(Some(v.value()));
    }
}

impl MaybeHolder {
    fn with_value(v: f64) -> Self {
        Self {
            inner: Cell::new(Some(v)),
        }
    }

    fn empty() -> Self {
        Self {
            inner: Cell::new(None),
        }
    }
}

// =============================================================================
// Prototype property — getter
// =============================================================================

#[test]
fn prototype_getter_returns_initial_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(42.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!((v.value() - 42.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn prototype_getter_reflects_rust_side_mutation() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        let r2 = r.clone();
        ctx.set_global("c", r.to_js(lock));
        r2.value.set(99.0);
        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!((v.value() - 99.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn prototype_getter_returns_string_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(0.0, "hello"));
        ctx.set_global("c", r.to_js(lock));
        let v: String = ctx.eval(lock, "c.label").unwrap();
        assert_eq!(v, "hello");
        Ok(())
    });
}

// =============================================================================
// Prototype property — setter
// =============================================================================

#[test]
fn prototype_setter_updates_value_visible_via_getter() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(0.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        ctx.eval_raw("c.value = 77").unwrap();
        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!((v.value() - 77.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn prototype_setter_mutation_visible_from_rust() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(0.0, "x"));
        let r2 = r.clone();
        ctx.set_global("c", r.to_js(lock));
        ctx.eval_raw("c.value = 55").unwrap();
        assert!((r2.value.get() - 55.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn prototype_setter_called_multiple_times() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(0.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        ctx.eval_raw("c.value = 1; c.value = 2; c.value = 3")
            .unwrap();
        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!((v.value() - 3.0).abs() < f64::EPSILON);
        Ok(())
    });
}

// =============================================================================
// Prototype property — read-only enforcement
// =============================================================================

#[test]
fn prototype_readonly_property_throws_in_strict_mode() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "lbl"));
        ctx.set_global("c", r.to_js(lock));
        let result = ctx.eval_raw("'use strict'; c.label = 'changed'");
        assert!(
            result.is_err(),
            "expected TypeError assigning to read-only prototype property"
        );
        Ok(())
    });
}

#[test]
fn prototype_readonly_property_silently_ignored_in_sloppy_mode() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "original"));
        ctx.set_global("c", r.to_js(lock));
        // In sloppy mode, the assignment is silently ignored; value unchanged.
        ctx.eval_raw("c.label = 'changed'").unwrap();
        let v: String = ctx.eval(lock, "c.label").unwrap();
        assert_eq!(v, "original");
        Ok(())
    });
}

// =============================================================================
// Prototype property — enumerability / prototype chain
// =============================================================================

#[test]
fn prototype_property_not_in_object_keys() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        let keys: String = ctx.eval(lock, "Object.keys(c).join(',')").unwrap();
        assert_eq!(
            keys, "",
            "prototype properties must not appear in Object.keys()"
        );
        Ok(())
    });
}

#[test]
fn prototype_property_found_by_in_operator() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        let found: bool = ctx.eval(lock, "'value' in c").unwrap();
        assert!(found, "'in' must find prototype properties");
        Ok(())
    });
}

#[test]
fn prototype_property_not_an_own_property() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        let own: bool = ctx
            .eval(lock, "Object.prototype.hasOwnProperty.call(c, 'value')")
            .unwrap();
        assert!(!own, "prototype property must NOT be an own property");
        Ok(())
    });
}

// =============================================================================
// Prototype property — multiple instances are independent
// =============================================================================

#[test]
fn prototype_property_independent_across_instances() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let a = jsg::Rc::new(Counter::new(10.0, "a"));
        let b = jsg::Rc::new(Counter::new(20.0, "b"));
        ctx.set_global("a", a.to_js(lock));
        ctx.set_global("b", b.to_js(lock));
        let va: Number = ctx.eval(lock, "a.value").unwrap();
        let vb: Number = ctx.eval(lock, "b.value").unwrap();
        assert!((va.value() - 10.0).abs() < f64::EPSILON);
        assert!((vb.value() - 20.0).abs() < f64::EPSILON);
        ctx.eval_raw("a.value = 99").unwrap();
        let va2: Number = ctx.eval(lock, "a.value").unwrap();
        let vb2: Number = ctx.eval(lock, "b.value").unwrap();
        assert!((va2.value() - 99.0).abs() < f64::EPSILON);
        assert!(
            (vb2.value() - 20.0).abs() < f64::EPSILON,
            "setting a.value must not affect b"
        );
        Ok(())
    });
}

// =============================================================================
// Prototype property — coexistence with methods and constants
// =============================================================================

#[test]
fn prototype_property_coexists_with_method() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(5.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        ctx.eval_raw("c.value = 7").unwrap();
        ctx.eval_raw("c.reset()").unwrap();
        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!(
            (v.value()).abs() < f64::EPSILON,
            "reset() must set value back to 0"
        );
        Ok(())
    });
}

#[test]
fn prototype_property_coexists_with_static_constant() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let constructor = jsg::resource::function_template_of::<Counter>(lock);
        ctx.set_global("Counter", constructor.into());
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));

        let v: Number = ctx.eval(lock, "c.value").unwrap();
        assert!((v.value() - 1.0).abs() < f64::EPSILON);

        let max: Number = ctx.eval(lock, "Counter.MAX_VALUE").unwrap();
        assert!((max.value() - 1_000_000.0).abs() < f64::EPSILON);
        Ok(())
    });
}

// =============================================================================
// Prototype property — camelCase name derivation
// =============================================================================

#[test]
fn prototype_property_multi_word_camel_case() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MultiWord::new("Alice", "Smith"));
        ctx.set_global("p", r.to_js(lock));
        let first: String = ctx.eval(lock, "p.firstName").unwrap();
        let last: String = ctx.eval(lock, "p.lastName").unwrap();
        assert_eq!(first, "Alice");
        assert_eq!(last, "Smith");
        Ok(())
    });
}

#[test]
fn prototype_property_multi_word_setter() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MultiWord::new("Alice", "Smith"));
        ctx.set_global("p", r.to_js(lock));
        ctx.eval_raw("p.firstName = 'Bob'").unwrap();
        let first: String = ctx.eval(lock, "p.firstName").unwrap();
        assert_eq!(first, "Bob");
        Ok(())
    });
}

#[test]
fn prototype_property_original_rust_name_not_exposed() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MultiWord::new("Alice", "Smith"));
        ctx.set_global("p", r.to_js(lock));
        // The raw Rust names (get_first_name, set_first_name) must not be visible.
        let undef: bool = ctx
            .eval(lock, "typeof p.getFirstName === 'undefined'")
            .unwrap();
        assert!(undef, "raw getter name must not be exposed");
        let undef2: bool = ctx
            .eval(lock, "typeof p.setFirstName === 'undefined'")
            .unwrap();
        assert!(undef2, "raw setter name must not be exposed");
        Ok(())
    });
}

// =============================================================================
// Prototype property — explicit name override
// =============================================================================

#[test]
fn explicit_name_override_getter() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(ExplicitName::new(42.0));
        ctx.set_global("obj", r.to_js(lock));
        let v: Number = ctx.eval(lock, "obj.myValue").unwrap();
        assert!((v.value() - 42.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn explicit_name_override_setter() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(ExplicitName::new(0.0));
        ctx.set_global("obj", r.to_js(lock));
        ctx.eval_raw("obj.myValue = 7").unwrap();
        let v: Number = ctx.eval(lock, "obj.myValue").unwrap();
        assert!((v.value() - 7.0).abs() < f64::EPSILON);
        Ok(())
    });
}

#[test]
fn explicit_name_original_rust_name_not_exposed() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(ExplicitName::new(0.0));
        ctx.set_global("obj", r.to_js(lock));
        let undef: bool = ctx
            .eval(lock, "typeof obj.getSomething === 'undefined'")
            .unwrap();
        assert!(undef);
        Ok(())
    });
}

// =============================================================================
// Prototype property — Option<T> return (null)
// =============================================================================

#[test]
fn prototype_option_property_returns_value_when_some() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MaybeHolder::with_value(std::f64::consts::PI));
        ctx.set_global("h", r.to_js(lock));
        let v: Number = ctx.eval(lock, "h.value").unwrap();
        assert!((v.value() - std::f64::consts::PI).abs() < 1e-10);
        Ok(())
    });
}

#[test]
fn prototype_option_property_returns_undefined_when_none() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MaybeHolder::empty());
        ctx.set_global("h", r.to_js(lock));
        // Option<T>::None maps to `undefined` via jsg::ToJS (same as a missing value).
        let is_nullish: bool = ctx.eval(lock, "h.value == null").unwrap();
        assert!(
            is_nullish,
            "None getter return must be nullish (null or undefined) in JS"
        );
        Ok(())
    });
}

#[test]
fn prototype_option_property_setter_makes_it_some() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(MaybeHolder::empty());
        ctx.set_global("h", r.to_js(lock));
        ctx.eval_raw("h.value = 2.5").unwrap();
        let v: Number = ctx.eval(lock, "h.value").unwrap();
        assert!((v.value() - 2.5).abs() < f64::EPSILON);
        Ok(())
    });
}

// =============================================================================
// Instance property — getter
// =============================================================================

#[test]
fn instance_getter_returns_initial_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("tok-123", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        let id: String = ctx.eval(lock, "t.id").unwrap();
        assert_eq!(id, "tok-123");
        Ok(())
    });
}

#[test]
fn instance_readonly_getter_returns_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("abc", "jwt"));
        ctx.set_global("t", r.to_js(lock));
        let kind: String = ctx.eval(lock, "t.kind").unwrap();
        assert_eq!(kind, "jwt");
        Ok(())
    });
}

// =============================================================================
// Instance property — setter
// =============================================================================

#[test]
fn instance_setter_updates_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("old", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        ctx.eval_raw("t.id = 'new-id'").unwrap();
        let id: String = ctx.eval(lock, "t.id").unwrap();
        assert_eq!(id, "new-id");
        Ok(())
    });
}

#[test]
fn instance_setter_visible_from_rust() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("orig", "bearer"));
        let r2 = r.clone();
        ctx.set_global("t", r.to_js(lock));
        ctx.eval_raw("t.id = 'updated'").unwrap();
        assert_eq!(*r2.id.borrow(), "updated");
        Ok(())
    });
}

// =============================================================================
// Instance property — own-property semantics
// =============================================================================

#[test]
fn instance_property_is_own_property() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("tok", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        let own: bool = ctx
            .eval(lock, "Object.prototype.hasOwnProperty.call(t, 'id')")
            .unwrap();
        assert!(own, "instance property must be an own property");
        Ok(())
    });
}

#[test]
fn instance_property_has_accessor_descriptor() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("tok", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        // An accessor descriptor has `get`/`set` keys, not `value`/`writable`.
        let has_get: bool = ctx
            .eval(
                lock,
                "typeof Object.getOwnPropertyDescriptor(t, 'id').get === 'function'",
            )
            .unwrap();
        let has_set: bool = ctx
            .eval(
                lock,
                "typeof Object.getOwnPropertyDescriptor(t, 'id').set === 'function'",
            )
            .unwrap();
        assert!(
            has_get,
            "instance property must have a getter in its descriptor"
        );
        assert!(
            has_set,
            "read-write instance property must have a setter in its descriptor"
        );
        Ok(())
    });
}

#[test]
fn instance_readonly_property_has_no_setter_in_descriptor() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("tok", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        let set_undef: bool = ctx
            .eval(
                lock,
                "typeof Object.getOwnPropertyDescriptor(t, 'kind').set === 'undefined'",
            )
            .unwrap();
        assert!(set_undef, "read-only instance property must have no setter");
        Ok(())
    });
}

#[test]
fn instance_readonly_property_throws_in_strict_mode() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Token::new("tok", "bearer"));
        ctx.set_global("t", r.to_js(lock));
        let result = ctx.eval_raw("'use strict'; t.kind = 'other'");
        assert!(
            result.is_err(),
            "expected error assigning to read-only instance property"
        );
        Ok(())
    });
}

#[test]
fn instance_property_independent_across_instances() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let a = jsg::Rc::new(Token::new("id-a", "bearer"));
        let b = jsg::Rc::new(Token::new("id-b", "bearer"));
        ctx.set_global("a", a.to_js(lock));
        ctx.set_global("b", b.to_js(lock));
        ctx.eval_raw("a.id = 'changed-a'").unwrap();
        let id_a: String = ctx.eval(lock, "a.id").unwrap();
        let id_b: String = ctx.eval(lock, "b.id").unwrap();
        assert_eq!(id_a, "changed-a");
        assert_eq!(id_b, "id-b", "mutating a.id must not affect b.id");
        Ok(())
    });
}

// =============================================================================
// Inspect property
// =============================================================================

#[test]
fn inspect_property_not_accessible_by_string_key() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(3.0, "dbg"));
        ctx.set_global("c", r.to_js(lock));
        let undef: bool = ctx
            .eval(lock, "typeof c.debugInfo === 'undefined'")
            .unwrap();
        assert!(
            undef,
            "inspect property must not be accessible via string key"
        );
        Ok(())
    });
}

#[test]
fn inspect_property_not_in_object_keys() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(3.0, "dbg"));
        ctx.set_global("c", r.to_js(lock));
        let keys: String = ctx.eval(lock, "Object.keys(c).join(',')").unwrap();
        assert!(
            !keys.contains("debugInfo"),
            "inspect property must not appear in Object.keys()"
        );
        Ok(())
    });
}

#[test]
fn inspect_property_not_in_own_string_names() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(3.0, "dbg"));
        ctx.set_global("c", r.to_js(lock));
        let names: String = ctx
            .eval(lock, "Object.getOwnPropertyNames(c).join(',')")
            .unwrap();
        assert!(
            !names.contains("debugInfo"),
            "inspect property must not appear in getOwnPropertyNames(), got: {names}"
        );
        Ok(())
    });
}

#[test]
fn inspect_property_registered_under_a_symbol() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(3.0, "dbg"));
        ctx.set_global("c", r.to_js(lock));
        // The property is registered under a v8::Symbol; at least one symbol must exist on
        // the prototype chain that has an accessor descriptor.
        let has_symbol_accessor: bool = ctx
            .eval(
                lock,
                r"
                (function() {
                    let proto = Object.getPrototypeOf(c);
                    let syms = Object.getOwnPropertySymbols(proto);
                    for (let sym of syms) {
                        let d = Object.getOwnPropertyDescriptor(proto, sym);
                        if (d && typeof d.get === 'function') return true;
                    }
                    return false;
                })()
                ",
            )
            .unwrap();
        assert!(
            has_symbol_accessor,
            "inspect property must be accessible via its symbol"
        );
        Ok(())
    });
}

#[test]
fn inspect_property_getter_returns_correct_value() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(7.0, "test"));
        ctx.set_global("c", r.to_js(lock));
        // Retrieve the symbol-keyed getter from the prototype and call it.
        let result: String = ctx
            .eval(
                lock,
                r"
                (function() {
                    let proto = Object.getPrototypeOf(c);
                    let syms = Object.getOwnPropertySymbols(proto);
                    for (let sym of syms) {
                        let d = Object.getOwnPropertyDescriptor(proto, sym);
                        if (d && typeof d.get === 'function') {
                            return d.get.call(c);
                        }
                    }
                    return 'NOT FOUND';
                })()
                ",
            )
            .unwrap();
        assert!(
            result.contains("Counter"),
            "inspect getter must return debug string, got: {result}"
        );
        assert!(
            result.contains('7'),
            "debug string must contain current value"
        );
        Ok(())
    });
}

#[test]
fn inspect_explicit_name_override() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(ExplicitName::new(5.0));
        ctx.set_global("obj", r.to_js(lock));
        // The inspect property must not be accessible as a string key "debugX".
        let undef: bool = ctx.eval(lock, "typeof obj.debugX === 'undefined'").unwrap();
        assert!(
            undef,
            "explicitly-named inspect property must still be hidden under a symbol"
        );
        Ok(())
    });
}

// =============================================================================
// Receiver guard — prototype properties must enforce correct `this`
// =============================================================================

#[test]
fn prototype_property_getter_throws_on_wrong_receiver() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        // Extract the getter from the descriptor and call it with a plain object receiver.
        let result = ctx.eval_raw(
            r"
            'use strict';
            let proto = Object.getPrototypeOf(c);
            let desc = Object.getOwnPropertyDescriptor(proto, 'value');
            desc.get.call({});
            ",
        );
        assert!(result.is_err(), "getter with wrong receiver must throw");
        Ok(())
    });
}

#[test]
fn prototype_property_setter_throws_on_wrong_receiver() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Counter::new(1.0, "x"));
        ctx.set_global("c", r.to_js(lock));
        let result = ctx.eval_raw(
            r"
            'use strict';
            let proto = Object.getPrototypeOf(c);
            let desc = Object.getOwnPropertyDescriptor(proto, 'value');
            desc.set.call({}, 99);
            ",
        );
        assert!(result.is_err(), "setter with wrong receiver must throw");
        Ok(())
    });
}

// =============================================================================
// Interplay: prototype vs instance properties on the same resource
// =============================================================================

/// A resource that mixes both kinds of properties.
#[jsg_resource]
struct Mixed {
    proto_val: Cell<f64>,
    instance_val: RefCell<String>,
}

#[jsg_resource]
impl Mixed {
    #[jsg_prototype_property]
    pub fn get_proto_val(&self) -> Number {
        Number::new(self.proto_val.get())
    }

    #[jsg_prototype_property]
    pub fn set_proto_val(&self, v: Number) {
        self.proto_val.set(v.value());
    }

    #[jsg_instance_property]
    pub fn get_instance_val(&self) -> String {
        self.instance_val.borrow().clone()
    }

    #[jsg_instance_property]
    pub fn set_instance_val(&self, v: String) {
        *self.instance_val.borrow_mut() = v;
    }
}

impl Mixed {
    fn new(p: f64, i: impl Into<String>) -> Self {
        Self {
            proto_val: Cell::new(p),
            instance_val: RefCell::new(i.into()),
        }
    }
}

#[test]
fn mixed_prototype_not_own_instance_is_own() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Mixed::new(1.0, "hello"));
        ctx.set_global("m", r.to_js(lock));

        let proto_own: bool = ctx
            .eval(lock, "Object.prototype.hasOwnProperty.call(m, 'protoVal')")
            .unwrap();
        let inst_own: bool = ctx
            .eval(
                lock,
                "Object.prototype.hasOwnProperty.call(m, 'instanceVal')",
            )
            .unwrap();

        assert!(!proto_own, "protoVal must NOT be an own property");
        assert!(inst_own, "instanceVal MUST be an own property");
        Ok(())
    });
}

#[test]
fn mixed_both_properties_readable() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Mixed::new(3.0, "world"));
        ctx.set_global("m", r.to_js(lock));
        let pv: Number = ctx.eval(lock, "m.protoVal").unwrap();
        let iv: String = ctx.eval(lock, "m.instanceVal").unwrap();
        assert!((pv.value() - 3.0).abs() < f64::EPSILON);
        assert_eq!(iv, "world");
        Ok(())
    });
}

#[test]
fn mixed_both_properties_writable() {
    let harness = crate::Harness::new();
    harness.run_in_context(|lock, ctx| {
        let r = jsg::Rc::new(Mixed::new(0.0, "old"));
        ctx.set_global("m", r.to_js(lock));
        ctx.eval_raw("m.protoVal = 9; m.instanceVal = 'new'")
            .unwrap();
        let pv: Number = ctx.eval(lock, "m.protoVal").unwrap();
        let iv: String = ctx.eval(lock, "m.instanceVal").unwrap();
        assert!((pv.value() - 9.0).abs() < f64::EPSILON);
        assert_eq!(iv, "new");
        Ok(())
    });
}
