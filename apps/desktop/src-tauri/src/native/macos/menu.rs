// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The spelling menu, an `NSMenu` laid out like the one in any Mac text
//! field. Guesses, Ignore Spelling and Learn Spelling report back to a small
//! target object. Cut, Copy and Paste have no target, so AppKit sends them
//! to the focused webview, which also decides whether they're enabled.

// Declaring the target class and giving menu items a selector and a target
// need `unsafe`.
#![allow(unsafe_code)]

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSMenu, NSMenuItem, NSView};
use objc2_foundation::{NSPoint, NSString};

/// What you picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picked {
    /// Nothing, or an edit command the webview carried out itself.
    Nothing,
    /// The guess at this index.
    Guess(usize),
    /// Ignore Spelling.
    Ignore,
    /// Learn Spelling.
    Learn,
}

/// The tag of an item that wasn't picked.
const NOTHING: isize = -1;
/// The tag of Ignore Spelling. Guesses use their index.
const IGNORE: isize = -2;
/// The tag of Learn Spelling.
const LEARN: isize = -3;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has
    // no `Drop` impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MaxHQSpellingMenuTarget"]
    #[ivars = Cell<isize>]
    struct Target;

    impl Target {
        /// Records which item you picked.
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            self.ivars().set(item.tag());
        }
    }
);

impl Target {
    /// A target that hasn't seen a pick yet.
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(NOTHING));
        // SAFETY: NSObject's init takes no arguments and returns the object.
        unsafe { msg_send![super(this), init] }
    }

    /// What the recorded tag means.
    fn picked(&self) -> Picked {
        match self.ivars().get() {
            IGNORE => Picked::Ignore,
            LEARN => Picked::Learn,
            index => usize::try_from(index).map_or(Picked::Nothing, Picked::Guess),
        }
    }
}

/// Shows the menu at `x`, `y` in the view, measured in points from its top
/// left corner, and returns once it closes. `guesses` is set when the menu
/// is for a misspelled word.
pub fn pop_up(
    view: &NSView,
    guesses: Option<&[String]>,
    x: f64,
    y: f64,
    labels: &crate::native::SpellingMenuLabels,
) -> Picked {
    let mtm = MainThreadMarker::from(view);
    let target = Target::new(mtm);
    let menu = NSMenu::new(mtm);
    if let Some(guesses) = guesses {
        if guesses.is_empty() {
            // With no action, AppKit shows it disabled.
            menu.addItem(&item(
                mtm,
                &NSString::from_str(&labels.no_guesses),
                None,
                None,
                NOTHING,
            ));
        }
        for (index, guess) in guesses.iter().enumerate() {
            let tag = isize::try_from(index).unwrap_or(NOTHING);
            menu.addItem(&item(
                mtm,
                &NSString::from_str(guess),
                Some(sel!(pick:)),
                Some(&target),
                tag,
            ));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&item(
            mtm,
            &NSString::from_str(&labels.ignore),
            Some(sel!(pick:)),
            Some(&target),
            IGNORE,
        ));
        menu.addItem(&item(
            mtm,
            &NSString::from_str(&labels.learn),
            Some(sel!(pick:)),
            Some(&target),
            LEARN,
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    menu.addItem(&item(
        mtm,
        &NSString::from_str(&labels.cut),
        Some(sel!(cut:)),
        None,
        NOTHING,
    ));
    menu.addItem(&item(
        mtm,
        &NSString::from_str(&labels.copy),
        Some(sel!(copy:)),
        None,
        NOTHING,
    ));
    menu.addItem(&item(
        mtm,
        &NSString::from_str(&labels.paste),
        Some(sel!(paste:)),
        None,
        NOTHING,
    ));

    let y = if view.isFlipped() {
        y
    } else {
        view.bounds().size.height - y
    };
    menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), Some(view));
    target.picked()
}

/// A menu item. Without a target, AppKit sends the action to whatever has
/// focus.
fn item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: Option<Sel>,
    target: Option<&Target>,
    tag: isize,
) -> Retained<NSMenuItem> {
    // SAFETY: `pick:` is defined on the target above, and `cut:`, `copy:`
    // and `paste:` are the standard edit actions.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            title,
            action,
            &NSString::from_str(""),
        )
    };
    let target: Option<&AnyObject> = target.map(|target| target.as_ref());
    // SAFETY: The target answers the item's action, and it outlives the
    // menu, which holds it weakly.
    unsafe { item.setTarget(target) };
    item.setTag(tag);
    item
}
