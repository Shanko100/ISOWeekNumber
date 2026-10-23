"""A minimal Objective-C bridge for the macOS backend, over ctypes.

PyObjC, which this replaces, costs about 20 MB at import -- more than a third
of the app at rest -- to make the two dozen AppKit calls WeekNumber needs.
This module sends each message through objc_msgSend with that call's exact C
signature, reads AppKit's global constants, and defines the one Objective-C
class the menu-bar item needs as a target. Loading the frameworks themselves
costs well under a megabyte, and Tk loads AppKit anyway.

Cocoa's ownership rules apply as written: what `alloc`/`new`/`copy` returns
belongs to the caller, who releases it; everything else is autoreleased, so
callers wrap their calls in `autorelease_pool()`. Object pointers are plain
ints (None for nil), as ctypes returns them for c_void_p.
"""

import ctypes
import platform
from contextlib import contextmanager
from ctypes import c_bool, c_char_p, c_double, c_size_t, c_ulong, c_void_p

from weeknumber_platform._common import log

#: An Objective-C object, class or selector pointer.
c_id = c_void_p


class NSPoint(ctypes.Structure):
    _fields_ = [("x", c_double), ("y", c_double)]


class NSSize(ctypes.Structure):
    _fields_ = [("width", c_double), ("height", c_double)]


class NSRect(ctypes.Structure):
    _fields_ = [("origin", NSPoint), ("size", NSSize)]


class NSEdgeInsets(ctypes.Structure):
    _fields_ = [("top", c_double), ("left", c_double),
                ("bottom", c_double), ("right", c_double)]


_objc = ctypes.CDLL("/usr/lib/libobjc.A.dylib")
CORE_FOUNDATION = ctypes.CDLL(
    "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
FOUNDATION = ctypes.CDLL("/System/Library/Frameworks/Foundation.framework/Foundation")
APPKIT = ctypes.CDLL("/System/Library/Frameworks/AppKit.framework/AppKit")

_objc.objc_getClass.restype = c_id
_objc.objc_getClass.argtypes = (c_char_p,)
_objc.sel_registerName.restype = c_id
_objc.sel_registerName.argtypes = (c_char_p,)
_objc.objc_allocateClassPair.restype = c_id
_objc.objc_allocateClassPair.argtypes = (c_id, c_char_p, c_size_t)
_objc.objc_registerClassPair.argtypes = (c_id,)
_objc.class_addMethod.restype = c_bool
_objc.class_addMethod.argtypes = (c_id, c_id, c_void_p, c_char_p)
_objc.objc_autoreleasePoolPush.restype = c_void_p
_objc.objc_autoreleasePoolPop.argtypes = (c_void_p,)

_SEND = ctypes.cast(_objc.objc_msgSend, c_void_p).value
# Intel returns structs over 16 bytes (NSRect) through a hidden pointer, which
# needs the _stret entry point; Apple silicon's objc_msgSend handles them all.
_SEND_STRET = (ctypes.cast(_objc.objc_msgSend_stret, c_void_p).value
               if platform.machine() == "x86_64" else None)

_selectors: "dict[str, int]" = {}
_senders: dict = {}


def sel(name: str) -> int:
    selector = _selectors.get(name)
    if selector is None:
        selector = _selectors[name] = _objc.sel_registerName(name.encode("ascii"))
    return selector


def cls(name: str) -> "int | None":
    """The class named `name`, or None when this macOS lacks it."""
    return _objc.objc_getClass(name.encode("ascii"))


def send(receiver, selector: str, *args, restype=c_id, argtypes=()):
    """[receiver selector...] with `args`, called as the C function the
    method really is: (id self, SEL _cmd, *argtypes) -> restype."""
    key = (restype, tuple(argtypes))
    function = _senders.get(key)
    if function is None:
        address = _SEND
        if (_SEND_STRET is not None and isinstance(restype, type)
                and issubclass(restype, ctypes.Structure) and ctypes.sizeof(restype) > 16):
            address = _SEND_STRET
        function = _senders[key] = ctypes.CFUNCTYPE(restype, c_id, c_id, *argtypes)(address)
    return function(receiver, sel(selector), *args)


def new(class_name: str, init: str = "init", *args, argtypes=()) -> "int | None":
    """[[class alloc] init...]: owned by the caller, who releases it."""
    return send(send(cls(class_name), "alloc"), init, *args, argtypes=argtypes)


def retain(obj):
    return send(obj, "retain") if obj else obj


def release(obj) -> None:
    if obj:
        send(obj, "release", restype=None)


@contextmanager
def autorelease_pool():
    token = _objc.objc_autoreleasePoolPush()
    try:
        yield
    finally:
        _objc.objc_autoreleasePoolPop(token)


def nsstring(text: str) -> int:
    """An autoreleased NSString."""
    return send(cls("NSString"), "stringWithUTF8String:", text.encode("utf-8"),
                argtypes=(c_char_p,))


def to_str(obj) -> "str | None":
    raw = send(obj, "UTF8String", restype=c_char_p) if obj else None
    return None if raw is None else raw.decode("utf-8", "replace")


def nsdata(payload: bytes) -> int:
    """An autoreleased NSData holding a copy of `payload`."""
    return send(cls("NSData"), "dataWithBytes:length:", payload, len(payload),
                argtypes=(c_char_p, c_ulong))


def items(array) -> "list[int]":
    """The objects of an NSArray."""
    count = send(array, "count", restype=c_ulong) if array else 0
    return [send(array, "objectAtIndex:", i, argtypes=(c_ulong,)) for i in range(count)]


def responds(obj, selector: str) -> bool:
    return bool(send(obj, "respondsToSelector:", sel(selector),
                     restype=c_bool, argtypes=(c_id,)))


def constant(name: str) -> int:
    """An AppKit `NSString *` constant, such as NSFontAttributeName."""
    return c_id.in_dll(APPKIT, name).value


def float_constant(name: str) -> float:
    """An AppKit CGFloat constant, such as NSFontWeightBold."""
    return c_double.in_dll(APPKIT, name).value


#: - (void)method:(id)argument, the shape of every method define_class adds.
_METHOD = ctypes.CFUNCTYPE(None, c_id, c_id, c_id)
#: The method implementations: Objective-C keeps raw pointers to them.
_methods: list = []


def define_class(name: str, methods: "dict[str, object]") -> int:
    """An NSObject subclass whose methods -- selector: f(self, argument) --
    each take one object and return nothing. Defined once per process:
    Objective-C classes cannot be removed, so a second call returns the
    first class, and `methods` should dispatch through state of their own."""
    existing = cls(name)
    if existing:
        return existing
    new_class = _objc.objc_allocateClassPair(cls("NSObject"), name.encode("ascii"), 0)

    def bind(function):
        def method(this, _cmd, argument):
            try:
                function(this, argument)
            except Exception:  # an exception must not cross into Objective-C
                log.exception("%s failed", name)
        return method

    for selector, function in methods.items():
        implementation = _METHOD(bind(function))
        _methods.append(implementation)
        _objc.class_addMethod(new_class, sel(selector),
                              ctypes.cast(implementation, c_void_p), b"v@:@")
    _objc.objc_registerClassPair(new_class)
    return new_class


# ---------------------------------------------------------------------------
# CoreFoundation property lists (managed preferences)
# ---------------------------------------------------------------------------
_cf = CORE_FOUNDATION
_cf.CFStringCreateWithCString.restype = c_void_p
_cf.CFStringCreateWithCString.argtypes = (c_void_p, c_char_p, ctypes.c_uint32)
_cf.CFRelease.argtypes = (c_void_p,)
_cf.CFGetTypeID.restype = c_ulong
_cf.CFGetTypeID.argtypes = (c_void_p,)
for _getter in ("CFBooleanGetTypeID", "CFNumberGetTypeID", "CFStringGetTypeID"):
    getattr(_cf, _getter).restype = c_ulong
_cf.CFBooleanGetValue.restype = ctypes.c_ubyte
_cf.CFBooleanGetValue.argtypes = (c_void_p,)
_cf.CFNumberIsFloatType.restype = ctypes.c_ubyte
_cf.CFNumberIsFloatType.argtypes = (c_void_p,)
_cf.CFNumberGetValue.restype = ctypes.c_ubyte
_cf.CFNumberGetValue.argtypes = (c_void_p, ctypes.c_long, c_void_p)
_cf.CFPreferencesCopyAppValue.restype = c_void_p
_cf.CFPreferencesCopyAppValue.argtypes = (c_void_p, c_void_p)
_cf.CFPreferencesAppValueIsForced.restype = ctypes.c_ubyte
_cf.CFPreferencesAppValueIsForced.argtypes = (c_void_p, c_void_p)

_UTF8 = 0x08000100        # kCFStringEncodingUTF8
_SINT64 = 4               # kCFNumberSInt64Type
_FLOAT64 = 6              # kCFNumberFloat64Type


class _CFString:
    """A CFString for the length of a `with` block."""

    def __init__(self, text: str) -> None:
        self.ref = _cf.CFStringCreateWithCString(None, text.encode("utf-8"), _UTF8)

    def __enter__(self) -> int:
        return self.ref

    def __exit__(self, *exc) -> None:
        if self.ref:
            _cf.CFRelease(self.ref)


def _python_value(ref) -> object:
    """A property-list value as bool, int, float or str; None otherwise."""
    kind = _cf.CFGetTypeID(ref)
    if kind == _cf.CFBooleanGetTypeID():
        return bool(_cf.CFBooleanGetValue(ref))
    if kind == _cf.CFNumberGetTypeID():
        if _cf.CFNumberIsFloatType(ref):
            value = c_double()
            return value.value if _cf.CFNumberGetValue(ref, _FLOAT64, ctypes.byref(value)) else None
        number = ctypes.c_int64()
        return number.value if _cf.CFNumberGetValue(ref, _SINT64, ctypes.byref(number)) else None
    if kind == _cf.CFStringGetTypeID():
        return to_str(ref)  # toll-free bridged with NSString
    return None


def preference(key: str, domain: str) -> object:
    """CFPreferencesCopyAppValue, converted (see _python_value)."""
    with _CFString(key) as cf_key, _CFString(domain) as cf_domain:
        ref = _cf.CFPreferencesCopyAppValue(cf_key, cf_domain)
    if not ref:
        return None
    try:
        return _python_value(ref)
    finally:
        _cf.CFRelease(ref)


def preference_forced(key: str, domain: str) -> bool:
    with _CFString(key) as cf_key, _CFString(domain) as cf_domain:
        return bool(_cf.CFPreferencesAppValueIsForced(cf_key, cf_domain))
