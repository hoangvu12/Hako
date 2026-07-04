/**
 * The single source of truth for every TanStack Query key in the app.
 *
 * Query keys were previously scattered as raw `["clips"]`-style literals across
 * hooks and components (and `["clips"]` had even been defined twice, in
 * `use-library.ts` and the cloud `keys.ts`, already at risk of drifting). A
 * typo'd literal doesn't error — it just silently reads/writes a different cache
 * bucket, so a mutation's `invalidateQueries` quietly stops matching its query.
 * Centralizing here makes every key a checked reference.
 *
 * Static keys are `readonly` tuples; parameterized keys are builder functions.
 */
export const queryKeys = {
  /** The clip library (newest-first). */
  clips: ["clips"] as const,
  /** One clip's audio tracks (count + names), keyed by clip id. */
  clipAudioTracks: (id: number) => ["clip-audio-tracks", id] as const,

  /** Persisted app settings. */
  settings: ["settings"] as const,

  /** Recorder status (buffer/recording state). */
  recorderStatus: ["recorder-status"] as const,
  /** Live Valorant presence/match status. */
  valorantStatus: ["valorant-status"] as const,

  /** Bundled Valorant agent/map artwork. */
  valorantAssets: ["valorant-assets"] as const,
  /** Bundled League champion/map artwork. */
  lolAssets: ["lol-assets"] as const,

  /** User-added custom games (generic "record any game" list). */
  customGames: ["custom-games"] as const,
  /** Capturable top-level windows (the Request-a-Game picker). */
  windows: ["windows"] as const,

  /** Available GPU adapters. */
  gpuInfo: ["gpu-info"] as const,
  /** FFmpeg availability/capabilities. */
  ffmpegInfo: ["ffmpeg-info"] as const,

  /** Microphone input devices. */
  audioInputs: ["audio-inputs"] as const,
  /** System audio output devices. */
  audioOutputs: ["audio-outputs"] as const,
  /** Per-application audio sessions (app-audio capture). */
  audioSessions: ["audio-sessions"] as const,
  /** Whether per-process loopback capture is supported on this OS build. */
  processLoopbackSupported: ["process-loopback-supported"] as const,

  /** Configured cloud providers. */
  cloudProviders: ["cloud-providers"] as const,
  /** In-flight + recent cloud uploads. */
  cloudUploads: ["cloud-uploads"] as const,
  /** Local-cache retention stats (cloud "free up space"). */
  cloudRetention: ["cloud-retention"] as const,
} as const;
