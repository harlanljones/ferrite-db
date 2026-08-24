# Synchronous core, async facade later

The design doc sketched `async fn` traits, but Ferrite DB's compute runs on its own Rayon pool and an embedded library must not force an executor on hosts. The public core API is synchronous and blocking; an async facade arrives later as a thin bridge over the internal channels. This deliberately deviates from the doc's sketch — expect pressure to "fix" it.
