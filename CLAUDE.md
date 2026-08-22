# CLAUDE.md

Follow all repository instructions in `AGENTS.md`.

## Required Runtime

Run the complete application with `npm run dev`. This builds and starts the Docker runtime in the foreground because DTM processing requires the GDAL executables installed in the image.

Do not deploy a worktree development instance with `--detach`, `-d`, or a restart policy. The foreground runner traps normal exit, terminal closure, and interruption signals, stops the exact container it started, and uses `--rm` to remove it. Press `Ctrl+C` before ending work normally. A cached Docker image may remain afterward, but an image is inert and is not a running service.

The host-only scripts are for focused debugging and must not be used to validate raster downloads or processing unless GDAL is installed separately on the host.
