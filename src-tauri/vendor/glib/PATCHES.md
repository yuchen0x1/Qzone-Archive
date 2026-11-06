# Vendored GLib patch

This directory contains the upstream `glib` 0.18.5 crate, distributed under the included MIT license. It is used because Tauri's current GTK/WebKit dependency chain requires GLib 0.18, while the upstream fix is only released in GLib 0.20.

The only source change is the `VariantStrIter::impl_get` pointer-mutability fix from upstream gtk-rs commit [`05dff0ee696f9bcd8617cd48c4b812d046d440cb`](https://github.com/gtk-rs/gtk-rs-core/commit/05dff0ee696f9bcd8617cd48c4b812d046d440cb): the output pointer is mutable and passed as `&mut p` to `g_variant_get_child`, rather than passing an immutable reference to a function that writes through it.

Remove this vendored override when the GTK/WebKit dependency chain can use a released, compatible GLib version containing the fix.
