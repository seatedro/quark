# quark-render

The wgpu renderer for [Quark](../../README.md) scenes. A `GpuContext` holds
the GPU state all windows share; each window's `Renderer` draws
`quark::Scene`s into its surface or into offscreen targets.

Backends are Vulkan, Metal, and DX12, with GLES on Linux for machines
without a Vulkan driver. `WGPU_BACKEND` picks one. The `headless-render`
feature renders without a window, for tests that read pixels back.

Apps reach this crate through `quark-app`.
