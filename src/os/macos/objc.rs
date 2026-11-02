//! The Objective-C runtime and the C frameworks, called directly: message
//! sends with exact prototypes (Apple silicon's calling convention needs
//! them), class definitions for the few AppKit subclasses the app needs,
//! and Core Foundation helpers. This and the other macOS backend files are
//! the only places `unsafe` is allowed outside the Windows backend; every
//! call goes through a small wrapper that states what it relies on.
#![allow(unsafe_code)]
#![allow(non_upper_case_globals, dead_code)]

use std::ffi::{CStr, c_char, c_void};

pub type Id = *mut c_void;
pub type Sel = *const c_void;
pub type Class = *mut c_void;
pub const NIL: Id = std::ptr::null_mut();

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NSEdgeInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

impl CGRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: w,
                height: h,
            },
        }
    }
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Class;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_allocateClassPair(superclass: Class, name: *const c_char, extra: usize) -> Class;
    fn objc_registerClassPair(cls: Class);
    fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
    fn class_addIvar(
        cls: Class,
        name: *const c_char,
        size: usize,
        align: u8,
        types: *const c_char,
    ) -> bool;
    fn class_getInstanceVariable(cls: Class, name: *const c_char) -> *mut c_void;
    fn ivar_getOffset(ivar: *mut c_void) -> isize;
    fn object_getClass(obj: Id) -> Class;
    fn class_addProtocol(cls: Class, protocol: *mut c_void) -> bool;
    fn objc_getProtocol(name: *const c_char) -> *mut c_void;
    pub fn objc_autoreleasePoolPush() -> *mut c_void;
    pub fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    pub fn NSBeep();
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub fn CFRelease(cf: *const c_void);
    pub fn CFRetain(cf: *const c_void) -> *const c_void;
    pub fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: bool,
    ) -> *const c_void;
    pub fn CFDataCreate(alloc: *const c_void, bytes: *const u8, len: isize) -> *const c_void;
    pub fn CFDictionaryCreate(
        alloc: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        n: isize,
        key_cb: *const c_void,
        value_cb: *const c_void,
    ) -> *const c_void;
    pub fn CFAttributedStringCreate(
        alloc: *const c_void,
        s: *const c_void,
        attrs: *const c_void,
    ) -> *const c_void;
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    pub fn CGColorSpaceRelease(cs: *const c_void);
    pub fn CGDataProviderCreateWithCFData(data: *const c_void) -> *const c_void;
    pub fn CGDataProviderRelease(p: *const c_void);
    pub fn CGImageCreate(
        w: usize,
        h: usize,
        bpc: usize,
        bpp: usize,
        stride: usize,
        cs: *const c_void,
        info: u32,
        provider: *const c_void,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> *const c_void;
    pub fn CGImageRelease(img: *const c_void);
    pub fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bpc: usize,
        stride: usize,
        cs: *const c_void,
        info: u32,
    ) -> *mut c_void;
    pub fn CGContextRelease(c: *mut c_void);
    pub fn CGContextSetTextPosition(c: *mut c_void, x: f64, y: f64);
    pub fn CGContextSetGrayFillColor(c: *mut c_void, gray: f64, alpha: f64);
    pub fn CGContextSetShouldSmoothFonts(c: *mut c_void, v: bool);
    pub fn CGContextSetAllowsFontSubpixelPositioning(c: *mut c_void, v: bool);
    pub fn CGContextSetShouldSubpixelPositionFonts(c: *mut c_void, v: bool);
    pub fn CGPathApply(
        path: *const c_void,
        info: *mut c_void,
        f: extern "C" fn(*mut c_void, *const CGPathElement),
    );
    pub fn CGPathRelease(path: *const c_void);
    pub fn CGMainDisplayID() -> u32;
}

#[repr(C)]
pub struct CGPathElement {
    pub kind: i32,
    pub points: *const CGPoint,
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    pub fn CTFontCreateWithName(
        name: *const c_void,
        size: f64,
        matrix: *const c_void,
    ) -> *const c_void;
    pub fn CTFontCreateUIFontForLanguage(
        kind: u32,
        size: f64,
        lang: *const c_void,
    ) -> *const c_void;
    pub fn CTFontCreateCopyWithSymbolicTraits(
        font: *const c_void,
        size: f64,
        matrix: *const c_void,
        value: u32,
        mask: u32,
    ) -> *const c_void;
    pub fn CTFontGetAscent(font: *const c_void) -> f64;
    pub fn CTFontGetDescent(font: *const c_void) -> f64;
    pub fn CTFontGetGlyphsForCharacters(
        font: *const c_void,
        chars: *const u16,
        glyphs: *mut u16,
        n: isize,
    ) -> bool;
    pub fn CTFontCreatePathForGlyph(
        font: *const c_void,
        glyph: u16,
        matrix: *const c_void,
    ) -> *const c_void;
    pub fn CTFontCopyFamilyName(font: *const c_void) -> *const c_void;
    pub fn CTLineCreateWithAttributedString(s: *const c_void) -> *const c_void;
    pub fn CTLineGetTypographicBounds(
        line: *const c_void,
        a: *mut f64,
        d: *mut f64,
        l: *mut f64,
    ) -> f64;
    pub fn CTLineDraw(line: *const c_void, ctx: *mut c_void);
    pub static kCTFontAttributeName: *const c_void;
    pub static kCTForegroundColorFromContextAttributeName: *const c_void;
}

#[link(name = "System")]
unsafe extern "C" {
    pub static _dispatch_main_q: c_void;
    pub fn dispatch_async_f(
        queue: *const c_void,
        ctx: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

pub fn class(name: &CStr) -> Class {
    // SAFETY: a NUL-terminated name; returns nil for an unknown class.
    unsafe { objc_getClass(name.as_ptr()) }
}

pub fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated name; selectors are interned for the
    // process's life.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// Message sends with fixed prototypes. Each is safe to call with an
/// object that responds to the selector with exactly that signature, which
/// the callers in this backend guarantee.
macro_rules! sends {
    ($($name:ident ( $($a:ident : $t:ident),* );)*) => {
        $(
            /// # Safety
            /// `obj` must be nil or an object answering `sel` with this
            /// argument and return layout.
            #[allow(clippy::too_many_arguments)]
            pub unsafe fn $name<R, $($t),*>(obj: Id, sel: &CStr, $($a: $t),*) -> R {
                let s = crate::os::macos::objc::sel(sel);
                // SAFETY: objc_msgSend is called through a pointer cast to
                // the method's real prototype, as the runtime requires.
                unsafe {
                    let f: extern "C" fn(Id, Sel, $($t),*) -> R =
                        std::mem::transmute(objc_msgSend as *const c_void);
                    f(obj, s, $($a),*)
                }
            }
        )*
    };
}

sends! {
    send0();
    send1(a: A);
    send2(a: A, b: B);
    send3(a: A, b: B, c: C);
    send4(a: A, b: B, c: C, d: D);
    send5(a: A, b: B, c: C, d: D, e: E);
    send9(a: A, b: B, c: C, d: D, e: E, f: F, g: G, h: H, i: I);
}

/// [[cls alloc] init], +1.
pub fn new(cls: &CStr) -> Id {
    // SAFETY: alloc/init on a class that exists (checked by the caller's
    // use of AppKit/Foundation classes present since macOS 10.x).
    unsafe {
        let obj: Id = send0(class(cls), c"alloc");
        send0(obj, c"init")
    }
}

pub fn release(obj: Id) {
    if !obj.is_null() {
        // SAFETY: balances an owned (+1) reference.
        unsafe { send0::<()>(obj, c"release") }
    }
}

pub fn retain(obj: Id) -> Id {
    if obj.is_null() {
        return obj;
    }
    // SAFETY: takes a reference on a live object.
    unsafe { send0(obj, c"retain") }
}

/// An autoreleased NSString.
pub fn nsstring(s: &str) -> Id {
    // SAFETY: bytes and length passed together; UTF-8 encoding (4).
    unsafe {
        let obj: Id = send0(class(c"NSString"), c"alloc");
        let obj: Id = send3(
            obj,
            c"initWithBytes:length:encoding:",
            s.as_ptr(),
            s.len(),
            4usize,
        );
        send0(obj, c"autorelease")
    }
}

/// An NSString's text.
pub fn string_of(ns: Id) -> String {
    if ns.is_null() {
        return String::new();
    }
    // SAFETY: UTF8String returns a NUL-terminated buffer owned by the
    // string, copied out at once.
    unsafe {
        let p: *const c_char = send0(ns, c"UTF8String");
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// A CFString (+1) for Core Text calls.
pub fn cfstring(s: &str) -> *const c_void {
    // SAFETY: bytes and length passed together; kCFStringEncodingUTF8.
    unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            0x0800_0100,
            false,
        )
    }
}

pub fn cf_release(cf: *const c_void) {
    if !cf.is_null() {
        // SAFETY: balances a Create/Copy reference.
        unsafe { CFRelease(cf) }
    }
}

/// A subclass under construction.
pub struct ClassBuilder(Class);

impl ClassBuilder {
    pub fn new(name: &CStr, superclass: &CStr) -> Option<ClassBuilder> {
        let existing = class(name);
        if !existing.is_null() {
            return None;
        }
        // SAFETY: a fresh class name under an existing superclass.
        let c = unsafe { objc_allocateClassPair(class(superclass), name.as_ptr(), 0) };
        (!c.is_null()).then_some(ClassBuilder(c))
    }

    /// # Safety
    /// `imp` must be an `extern "C"` function whose prototype matches
    /// `types` (self, _cmd, then the arguments).
    pub unsafe fn method(&mut self, name: &CStr, imp: *const c_void, types: &CStr) {
        // SAFETY: as documented above.
        unsafe {
            class_addMethod(self.0, sel(name), imp, types.as_ptr());
        }
    }

    pub fn ivar_usize(&mut self, name: &CStr) {
        // SAFETY: an 8-byte, 8-aligned ivar with its type encoding.
        unsafe {
            class_addIvar(self.0, name.as_ptr(), 8, 3, c"Q".as_ptr());
        }
    }

    pub fn protocol(&mut self, name: &CStr) {
        // SAFETY: looks up a protocol by name; nil is ignored.
        unsafe {
            let p = objc_getProtocol(name.as_ptr());
            if !p.is_null() {
                class_addProtocol(self.0, p);
            }
        }
    }

    pub fn register(self) -> Class {
        // SAFETY: registers the finished class once.
        unsafe { objc_registerClassPair(self.0) };
        self.0
    }
}

/// Read an 8-byte ivar of an instance of one of our classes.
pub fn get_ivar(obj: Id, name: &CStr) -> usize {
    if obj.is_null() {
        return 0;
    }
    // SAFETY: the ivar was declared 8 bytes wide on the object's class;
    // its offset comes from the runtime.
    unsafe {
        let iv = class_getInstanceVariable(object_getClass(obj), name.as_ptr());
        if iv.is_null() {
            return 0;
        }
        let off = ivar_getOffset(iv);
        *(obj.cast::<u8>().offset(off).cast::<usize>())
    }
}

pub fn set_ivar(obj: Id, name: &CStr, v: usize) {
    if obj.is_null() {
        return;
    }
    // SAFETY: as get_ivar.
    unsafe {
        let iv = class_getInstanceVariable(object_getClass(obj), name.as_ptr());
        if iv.is_null() {
            return;
        }
        let off = ivar_getOffset(iv);
        *(obj.cast::<u8>().offset(off).cast::<usize>()) = v;
    }
}

/// Run `f` on the main thread (from the instance listener's thread).
pub fn on_main_thread(f: extern "C" fn(*mut c_void)) {
    // SAFETY: the main queue lives for the process; no context is passed.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, std::ptr::null_mut(), f) }
}

/// An autorelease pool for one turn of the loop.
pub struct Pool(*mut c_void);

impl Pool {
    pub fn new() -> Pool {
        // SAFETY: push/pop are paired by Drop.
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: pops the pool this value pushed.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}
