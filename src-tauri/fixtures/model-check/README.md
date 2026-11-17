# Public development catalog

Only the `model-check` Cargo feature includes these files. Its executable refuses
to launch unless the application ID is `com.speakmore.modelcheck.20260916`.
Normal builds retain their original production trust and three catalog entries.

Regenerate with `node scripts/diagnostics/prepare-model-check-catalog.mjs`.
The signer uses an intentionally public fixture seed; it is not a secret and must
never become a production trust root. No production key file is read or written.
The candidate URL is a test placeholder, not a published download. Use local
import with the exact generated graph, original tokens and VAD.

`npm run desktop:model-check` builds an isolated macOS app with the ordinary UI,
IPC, model manager, SQLite, and packaged workers. Startup disables its recording
shortcuts and selects manual delivery; it does not request microphone or
Accessibility permission. No synthetic ASR result or alternative IPC command is
provided. This is an internal validation build, not a release artifact.

`node scripts/build-model-check.mjs --public-audio` is an explicit second mode
of the same isolated app. It enables `model-audio-check`, substitutes a pinned
public 300-second PCM source and simulated capture permissions, and identifies
this in both the window title and a persistent banner. Start/Finish/Cancel use
ordinary IPC and real processing; no microphone is opened. The displayed live
timer is not the duration of the fixture. Startup keeps polishing/history off
and processing local. This mode does not verify physical capture or TCC.
The ordinary build command disables its banner and excludes its source/feature.
