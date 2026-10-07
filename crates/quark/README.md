# quark

The core crate of quark, a native Rust UI framework. It holds the parts that
know nothing about windows, GPUs, or text shaping: reactive signals, scene
primitives, hit testing, the semantic frame used for focus and accessibility,
document selection, the animation table, and style data.

Quark borrows what makes web UI tractable (stable identity, keyed children,
event routing, focus and accessibility semantics, virtualization) without
emulating the DOM or CSS. Apps get explicit Rust APIs, predictable lifecycles,
and GPU-first output.
