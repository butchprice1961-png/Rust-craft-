# Benchmarks

RustCraft publishes only numbers it has measured, together with the machine
and settings used. Nothing here compares RustCraft with the official client:
no such comparison has been run yet, so RustCraft makes no claim to be faster
than vanilla Minecraft.

## How to run the benchmark

The native client has a benchmark mode:

```sh
cargo build --release -p rustcraft-client
target/release/rustcraft-client --game-dir /tmp/bench --world Bench --seed 42 \
    --render-distance 8 --benchmark 20
```

It works like this:

1. It opens the world (it creates a new one with the seed if the world does
   not exist). It waits until every chunk within the render distance is
   loaded and meshed, and then for 10 more frames.
2. It switches to flight, 30 blocks above the player, and flies east in a
   straight line at 2 blocks per second (0.1 block per tick), looking 20°
   down. New chunks load during the flight.
3. After the given number of seconds, it prints one JSON line and saves it
   to `<game-dir>/benchmarks/<time>.json`.

The JSON fields are:

| Field | Meaning |
|---|---|
| `seconds_to_loaded` | Time from start until every chunk within the render distance was loaded and meshed |
| `frames` | Frames rendered during the flight |
| `avg_fps` | Frames divided by flight time |
| `one_percent_low_fps` | Average FPS over the slowest 1% of frames |
| `p50_frame_ms`, `p99_frame_ms`, `max_frame_ms` | Frame time percentiles during the flight |
| `sections_drawn`, `triangles` | What the last frame drew after frustum culling |
| `gpu_mesh_mib` | Vertex and index buffers of all loaded sections, drawn or not |

The client also logs `playable N s after start`. That is the time until the
3×3 chunks around the player are ready, when the loading screen goes away
and the launcher hides its window.

## Results

### 2026-10-29: software rendering, no GPU

These numbers come from a cloud container with no GPU, so wgpu ran on Mesa's
llvmpipe, which draws on the CPU. They show that the benchmark works. They
say nothing about speed on a real graphics card.

- **Machine:** 4 vCPU Intel Xeon at 2.10 GHz, 15 GB RAM, Ubuntu 24.04
- **Graphics:** llvmpipe (LLVM 20.1.2, 256 bits), Vulkan, Mesa 25.2.8, on an Xvfb display
- **Build:** RustCraft 0.1.0, release profile, the commit that added this page
- **Settings:** 1280×720 window, VSync on, seed 42, new world, 20-second flight, built-in textures

| Render distance | Playable after | Fully loaded after | Avg FPS | 1% low FPS | p50 frame | p99 frame | Sections drawn | Triangles drawn | Mesh memory |
|---|---|---|---|---|---|---|---|---|---|
| 8 | 0.23 s | 15.1 s | 11.3 | 7.2 | 84.2 ms | 132.7 ms | 496 | 403,644 | 100.7 MiB |
| 12 | 0.25 s | 36.7 s | 5.7 | 3.6 | 169.3 ms | 259.8 ms | 1,063 | 966,034 | 217.3 MiB |

Each row is one run. An earlier pair of runs is not shown, because it used
a flight route whose speed depended on the frame rate; that bug was fixed
before these runs.

What the numbers show:

- **Loading is tied to the frame rate.** The client starts loading at most
  6 new chunks and meshing at most 48 sections per frame. At 12 FPS, that
  makes full loading slow. The faster the frame rate, the faster the world
  loads. Making loading independent of the frame rate is a possible
  improvement.
- **Mesh memory** is 32 bytes per vertex plus 4 bytes per index: 152 bytes
  for each quad. Smaller vertices and 16-bit indices could cut this by about
  half. Nobody has measured that yet.

## Not measured yet

- Any run on a real GPU (Windows, macOS or Linux)
- The official client on the same machine, the same world and the same route, which a fair comparison needs
- Memory use of the whole process
- Launcher start time and memory use

Contributions of measurements are welcome. Please include the full JSON
output, your GPU, driver, operating system and RustCraft commit, and run the
same command for each render distance.
