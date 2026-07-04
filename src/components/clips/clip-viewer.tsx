import * as React from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useNavigate } from "@tanstack/react-router";
import {
  X,
  SpeakerSimpleHigh,
  SpeakerSimpleX,
  CornersOut,
  CornersIn,
  Scissors,
  FloppyDisk,
  DownloadSimple,
  ArrowCounterClockwise,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  useClips,
  useDeleteClip,
  useRemuxClip,
  useRenameClip,
  useTrimClip,
} from "@/hooks/use-library";
import {
  useClipDownload,
  useClipRemoteUrl,
  useDownloadClip,
} from "@/hooks/use-cloud";
import type { ClipRecord, TrackVolume, TrimMode } from "@/lib/api";

import { STREAM_SCHEME } from "./clip-viewer/constants";
import { useStemMix } from "./clip-viewer/use-stem-mix";
import { useTrimEditor } from "./clip-viewer/use-trim-editor";
import { useClipKeyboard } from "./clip-viewer/use-clip-keyboard";
import { fmtClock, rulerStep } from "./clip-viewer/format";
import { formatTime } from "@/lib/format";
import {
  CtrlButton,
  NavArrow,
  OverlaySeekBar,
  Playhead,
  PlayPauseButton,
  SettingsButton,
  TimeReadout,
} from "./clip-viewer/player-controls";
import { FilmstripStrip, TrimHandle } from "./clip-viewer/filmstrip";
import { AudioSettingsPopover } from "./clip-viewer/audio-settings";
import { SaveDialog } from "./clip-viewer/save-dialog";
import { DetailsPanel, Kbd } from "./clip-viewer/details-panel";

export function ClipViewer({ clipId }: { clipId: string }) {
  const navigate = useNavigate();
  const { data: clips, isLoading } = useClips();
  const del = useDeleteClip();
  const rename = useRenameClip();
  const trim = useTrimClip();
  const remux = useRemuxClip();

  const list = clips ?? [];
  const index = list.findIndex((c) => String(c.id) === clipId);
  const clip = index >= 0 ? list[index] : undefined;
  const prev = index > 0 ? list[index - 1] : undefined;
  const next = index >= 0 && index < list.length - 1 ? list[index + 1] : undefined;

  const close = React.useCallback(() => navigate({ to: "/clips" }), [navigate]);
  const goto = React.useCallback(
    (c?: ClipRecord) =>
      c && navigate({ to: "/clips/$clipId", params: { clipId: String(c.id) } }),
    [navigate],
  );

  const handleDelete = React.useCallback(() => {
    if (!clip) return;
    if (!window.confirm(`Delete "${clip.title || "Untitled"}"? This removes the file.`))
      return;
    const fallback = next ?? prev;
    del.mutate(clip.id, {
      onSuccess: () => (fallback ? goto(fallback) : close()),
    });
  }, [clip, next, prev, del, goto, close]);

  // Run the export; on "copy" jump to the freshly-created clip. When `tracks`
  // is provided (a per-track audio mix), re-mux; otherwise it's a loss-less
  // stream-copy trim that keeps every existing audio track.
  const handleExport = React.useCallback(
    async (args: {
      start: number;
      end: number;
      dropAudio: boolean;
      tracks: TrackVolume[] | null;
      mode: TrimMode;
    }) => {
      if (!clip) return;
      const rec = args.tracks
        ? await remux.mutateAsync({
            id: clip.id,
            start: args.start,
            end: args.end,
            tracks: args.tracks,
            mode: args.mode,
          })
        : await trim.mutateAsync({
            id: clip.id,
            start: args.start,
            end: args.end,
            dropAudio: args.dropAudio,
            mode: args.mode,
          });
      if (args.mode === "copy") goto(rec);
    },
    [clip, trim, remux, goto],
  );

  return (
    <div className="fixed inset-x-0 bottom-0 top-12 z-50 flex bg-black/85 backdrop-blur-sm">
      {/* Backdrop click-to-close (content stops propagation) */}
      <button
        type="button"
        aria-label="Close"
        onClick={close}
        className="absolute inset-0 cursor-default"
      />

      {!clip ? (
        <div className="relative z-10 flex flex-1 items-center justify-center text-sm text-muted-foreground">
          {isLoading ? "Loading…" : "Clip not found. It may have been deleted."}
          <button
            type="button"
            onClick={close}
            className="absolute top-4 right-4 flex size-9 items-center justify-center rounded-full bg-white/10 text-white transition-colors hover:bg-white/20"
          >
            <X weight="bold" className="size-4" />
          </button>
        </div>
      ) : (
        <ViewerStage
          // size in the key → an in-place overwrite remounts the editor fresh.
          key={`${clip.id}:${clip.size_bytes}`}
          clip={clip}
          hasPrev={!!prev}
          hasNext={!!next}
          onPrev={() => goto(prev)}
          onNext={() => goto(next)}
          onClose={close}
          onDelete={handleDelete}
          onRename={(title) =>
            title && title !== clip.title && rename.mutate({ id: clip.id, title })
          }
          onExport={handleExport}
          exportPending={trim.isPending || remux.isPending}
          exportError={
            trim.error || remux.error ? String(trim.error ?? remux.error) : null
          }
        />
      )}
    </div>
  );
}

function ViewerStage({
  clip,
  hasPrev,
  hasNext,
  onPrev,
  onNext,
  onClose,
  onDelete,
  onRename,
  onExport,
  exportPending,
  exportError,
}: {
  clip: ClipRecord;
  hasPrev: boolean;
  hasNext: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onDelete: () => void;
  onRename: (title: string) => void;
  onExport: (args: {
    start: number;
    end: number;
    dropAudio: boolean;
    tracks: TrackVolume[] | null;
    mode: TrimMode;
  }) => Promise<void>;
  exportPending: boolean;
  exportError: string | null;
}) {
  const stageRef = React.useRef<HTMLDivElement>(null);
  const videoRef = React.useRef<HTMLVideoElement>(null);
  const barRef = React.useRef<HTMLDivElement>(null);

  // Cloud-only (evicted) clips have no local file — "free up space" deleted it.
  // Play them straight from the presigned remote URL (range-capable, so seeking
  // still works); local clips stream over our range-aware `hakoclip://` scheme.
  const cloudUrl = useClipRemoteUrl(clip.id);
  // "Download to edit": re-fetch the evicted file so trim/export can run locally.
  const download = useDownloadClip();
  const dl = useClipDownload(clip.id);
  // Cache-bust so an overwrite (same path, new bytes) actually reloads. The video
  // streams over our range-aware scheme; images stay on the plain asset protocol.
  const src = React.useMemo(
    () =>
      clip.evicted
        ? (cloudUrl ?? undefined)
        : `${convertFileSrc(clip.path, STREAM_SCHEME)}?v=${clip.size_bytes}`,
    [clip.evicted, cloudUrl, clip.path, clip.size_bytes],
  );
  const poster = clip.thumb_path
    ? `${convertFileSrc(clip.thumb_path)}?v=${clip.size_bytes}`
    : undefined;
  const filmstripUrl = clip.filmstrip_path
    ? `${convertFileSrc(clip.filmstrip_path)}?v=${clip.size_bytes}`
    : undefined;

  // Stored duration is the render-time fallback; the <video>'s reported duration
  // (genuinely new data) wins once loaded. Reading the prop directly avoids the
  // stale copy a `useState(clip.duration_secs)` would hold if `clip` changed.
  const [muted, setMuted] = React.useState(false);
  const [volume, setVolume] = React.useState(1);
  const [videoDuration, setVideoDuration] = React.useState<number | null>(null);
  const duration = videoDuration ?? clip.duration_secs;
  const [fullscreen, setFullscreen] = React.useState(false);
  const [saveOpen, setSaveOpen] = React.useState(false);
  // Speed lives in <SettingsButton> (it has no audio coupling), but mute/volume stay
  // here: they feed the live audio mixer's master gain *and* the "m" shortcut, so
  // isolating them safely needs a shared store rather than a ref bridge.

  // Trim selection + pointer handling (filmstrip/ruler drags, seek helpers).
  const {
    trimStart,
    trimEnd,
    setTrimStart,
    setTrimEnd,
    touched,
    setTouched,
    drag,
    seekToTime,
    onBarPointerMove,
    endDrag,
    startHandle,
    startSeek,
    onRulerPointerDown,
  } = useTrimEditor({
    videoRef,
    barRef,
    duration,
    clipDuration: clip.duration_secs,
  });

  // Multi-track audio: per-stem mute/solo/volume/denoise + live Web Audio mix.
  const {
    audioEnabled,
    setAudioEnabled,
    toggleAudio,
    stems,
    hasStems,
    ctlOf,
    setTrackCtl,
    onStemMute,
    onStemSolo,
    onStemVolume,
    onStemDenoise,
    soloActive,
    audibleStems,
    tracksEdited,
    liveMix,
    mixDecoding,
    denoisingIdx,
  } = useStemMix({
    clipId: clip.id,
    fileSize: clip.size_bytes,
    videoRef,
    muted,
    volume,
  });

  // Reflect muted/volume onto the element (React doesn't track these). While live
  // mixing is active the element stays muted (Web Audio carries the sound); the
  // top-bar mute/volume then drives the graph's master gain instead.
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.muted = liveMix || muted;
  }, [muted, liveMix]);
  React.useEffect(() => {
    if (videoRef.current) videoRef.current.volume = volume;
  }, [volume]);

  const togglePlay = React.useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      // Start playback from the selection if we're outside it.
      if (v.currentTime < trimStart - 0.05 || v.currentTime >= trimEnd - 0.05)
        v.currentTime = trimStart;
      void v.play().catch(() => {});
    } else v.pause();
  }, [trimStart, trimEnd]);

  const toggleFullscreen = React.useCallback(async () => {
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await stageRef.current?.requestFullscreen();
    } catch {
      /* fullscreen unavailable */
    }
  }, []);

  React.useEffect(() => {
    const onChange = () =>
      setFullscreen(document.fullscreenElement === stageRef.current);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  useClipKeyboard({
    hasPrev,
    hasNext,
    onPrev,
    onNext,
    onClose,
    onDelete,
    togglePlay,
    toggleFullscreen,
    saveOpen,
    setSaveOpen,
    trimStart,
    trimEnd,
    setTrimStart,
    setTrimEnd,
    setTouched,
    setMuted,
    videoRef,
  });

  const startPct = duration > 0 ? (trimStart / duration) * 100 : 0;
  const endPct = duration > 0 ? (trimEnd / duration) * 100 : 100;
  // Ruler ticks depend only on duration — memoize so playhead updates (which fire
  // a few times a second) don't rebuild these arrays on every render.
  const { ticks, minorTicks } = React.useMemo(() => {
    const step = rulerStep(duration);
    const ticks: number[] = [];
    for (let t = 0; t <= duration + 0.001; t += step) ticks.push(t);
    // Finer ticks (10 per major step) for the ruler's measure marks.
    const minorStep = step / 10;
    const minorTicks: { pct: number; major: boolean }[] = [];
    for (let i = 0; i * minorStep <= duration + 0.001; i++) {
      minorTicks.push({ pct: ((i * minorStep) / duration) * 100, major: i % 10 === 0 });
    }
    return { ticks, minorTicks };
  }, [duration]);

  const selDuration = trimEnd - trimStart;
  const edited =
    trimStart > 0.05 || trimEnd < duration - 0.05 || !audioEnabled || tracksEdited;

  return (
    <div className="relative z-10 flex min-w-0 flex-1">
      {/* ---- Stage (player + editor) ---- */}
      <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-4 p-6 pr-2">
        {/* Video + overlay controls (this element goes fullscreen) */}
        <div
          ref={stageRef}
          className={cn(
            "group/video relative flex min-h-0 w-full flex-1 items-center justify-center",
            fullscreen && "bg-black",
          )}
        >
          {/* Prev/next anchored to the player's own edges (hidden in fullscreen) */}
          {hasPrev && !fullscreen ? <NavArrow side="left" onClick={onPrev} /> : null}
          {hasNext && !fullscreen ? <NavArrow side="right" onClick={onNext} /> : null}

          <video
            ref={videoRef}
            src={src}
            poster={poster}
            // Seed the element's intrinsic size from the stored dimensions. A
            // <video> defaults to 300×150 until its poster/metadata loads, so
            // without this the player paints tiny on mount and then jumps to
            // full size — the "small for a moment, then zoom in" flash. With the
            // real aspect known up front, `max-w/h-full object-contain` fits it
            // at its final size from the first frame.
            width={clip.width || undefined}
            height={clip.height || undefined}
            autoPlay
            playsInline
            onClick={togglePlay}
            onLoadedMetadata={(e) => {
              const d = e.currentTarget.duration;
              if (Number.isFinite(d) && d > 0) {
                setVideoDuration(d);
                if (!touched) setTrimEnd(d);
              }
            }}
            onTimeUpdate={(e) => {
              const v = e.currentTarget;
              // The playhead/readout/seek bar track time via their own
              // `useVideoTime` subscription — nothing to push here. We only
              // confine playback to the selection: loop at the out point, and
              // never play through the trimmed-away head.
              if (!v.paused && !drag && (v.currentTime >= trimEnd - 0.02 || v.currentTime < trimStart))
                v.currentTime = trimStart;
            }}
            className={cn(
              "bg-black",
              fullscreen
                // Fill the whole screen, cropping to fit (no letterboxing).
                ? "absolute inset-0 size-full object-cover"
                : "max-h-full max-w-full rounded-lg object-contain shadow-2xl",
            )}
          />

          {/* Overlay control bar — always visible, with a seek bar on top (the
              filmstrip editor below is the precise scrubber; this is the quick one). */}
          <div className="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex flex-col gap-1.5 rounded-b-lg bg-gradient-to-t from-black/85 via-black/35 to-transparent px-4 pt-12 pb-3 text-white [&>*]:pointer-events-auto">
            <OverlaySeekBar
              videoRef={videoRef}
              start={trimStart}
              end={trimEnd}
              marks={clip.event_marks}
              onSeek={seekToTime}
            />
            <div className="flex items-center gap-3">
            <PlayPauseButton videoRef={videoRef} onToggle={togglePlay} />

            <div className="group/vol flex items-center gap-2">
              <CtrlButton
                label={muted ? "Unmute" : "Mute"}
                onClick={() => setMuted((m) => !m)}
              >
                {muted || volume === 0 ? (
                  <SpeakerSimpleX weight="fill" className="size-5" />
                ) : (
                  <SpeakerSimpleHigh weight="fill" className="size-5" />
                )}
              </CtrlButton>
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={muted ? 0 : volume}
                onChange={(e) => {
                  const v = Number(e.target.value);
                  setVolume(v);
                  setMuted(v === 0);
                }}
                aria-label="Volume"
                className="h-1 w-0 cursor-pointer appearance-none rounded-full bg-white/30 opacity-0 transition-[width,opacity] duration-200 outline-none group-hover/vol:w-20 group-hover/vol:opacity-100 [&::-webkit-slider-thumb]:size-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white"
              />
            </div>

            <TimeReadout videoRef={videoRef} duration={duration} />

            <span className="flex-1" />

            <SettingsButton videoRef={videoRef} />
            <CtrlButton
              label={fullscreen ? "Exit fullscreen" : "Fullscreen"}
              onClick={toggleFullscreen}
            >
              {fullscreen ? (
                <CornersIn weight="bold" className="size-5" />
              ) : (
                <CornersOut weight="bold" className="size-5" />
              )}
            </CtrlButton>
            </div>
          </div>
        </div>

        {/* ---- Trim editor (filmstrip + toolbar) ---- */}
        {!fullscreen ? (
          <div className="w-full max-w-6xl shrink-0">
            {/* Padded so the trim handles have room at the very ends */}
            <div className="px-4">
              {/* Ruler — click / drag anywhere here to move the playhead */}
              <div
                onPointerDown={onRulerPointerDown}
                onPointerMove={onBarPointerMove}
                onPointerUp={endDrag}
                className="group/ruler relative h-9 cursor-pointer touch-none select-none"
              >
                {ticks.map((t) => (
                  <span
                    key={t}
                    className="pointer-events-none absolute top-0 -translate-x-1/2 font-sans text-[11px] font-medium tabular-nums text-white"
                    style={{ left: `${(t / duration) * 100}%` }}
                  >
                    {formatTime(t)}
                  </span>
                ))}
                {/* measure strip — a contained "tape" surface so the ticks read
                    as a ruler instead of floating marks on the backdrop */}
                <div className="pointer-events-none absolute inset-x-0 bottom-0 h-4 overflow-hidden rounded-md bg-gradient-to-b from-white/[0.06] to-white/[0.015] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.06)]">
                  {minorTicks.map((m, i) => (
                    <span
                      key={i}
                      className={cn(
                        "absolute bottom-0 w-px -translate-x-1/2",
                        m.major ? "bg-white" : "bg-white/45",
                      )}
                      style={{ left: `${m.pct}%`, height: "45%" }}
                    />
                  ))}
                </div>
              </div>

              {/* Filmstrip — frames clipped; selection chrome on an unclipped layer */}
              <div className="relative mt-1.5 h-[50px] select-none">
                {/* Clipped frame strip */}
                <div
                  ref={barRef}
                  onPointerMove={onBarPointerMove}
                  onPointerUp={endDrag}
                  className="absolute inset-0 touch-none overflow-hidden rounded-lg border border-white/10 bg-black/40"
                >
                  {/* Frames — sliced out of the Rust sprite-sheet (no webview decode) */}
                  <FilmstripStrip sprite={filmstripUrl} poster={poster} />

                  {/* Subtle base veil over the whole strip (Medal-style) so the
                      white selection frame and the playhead read clearly against
                      the frames. The trimmed-away regions get an extra dim on top. */}
                  <div className="pointer-events-none absolute inset-0 bg-black/20" />

                  {/* Dim the trimmed-away regions (outside the selection) */}
                  <div
                    className="pointer-events-none absolute inset-y-0 left-0 bg-black/60"
                    style={{ width: `${startPct}%` }}
                  />
                  <div
                    className="pointer-events-none absolute inset-y-0 right-0 bg-black/60"
                    style={{ width: `${100 - endPct}%` }}
                  />
                </div>

                {/* Unclipped selection chrome: a cohesive rounded frame whose
                    sides ARE the grab handles, plus the playhead. */}
                <div className="pointer-events-none absolute inset-0">
                  {/* Selection frame */}
                  <div
                    className="absolute inset-y-0 rounded-lg border-[3px] border-white"
                    style={{ left: `${startPct}%`, right: `${100 - endPct}%` }}
                  />

                  {/* Handles integrated into the frame edges */}
                  <TrimHandle side="start" pct={startPct} onPointerDown={(e) => startHandle("start", e)} />
                  <TrimHandle side="end" pct={endPct} onPointerDown={(e) => startHandle("end", e)} />

                  {/* Draggable playhead (flag + needle) */}
                  <Playhead
                    videoRef={videoRef}
                    duration={duration}
                    onPointerDown={startSeek}
                  />
                </div>
              </div>
            </div>

            {/* Editor toolbar */}
            <div className="mt-3 flex items-center gap-2.5 px-4">
              <AudioSettingsPopover
                audioEnabled={audioEnabled}
                onToggleAudio={toggleAudio}
                hasStems={hasStems}
                stems={stems}
                decoding={mixDecoding}
                denoisingIdx={denoisingIdx}
                ctlOf={ctlOf}
                soloActive={soloActive}
                onMute={onStemMute}
                onSolo={onStemSolo}
                onVolume={onStemVolume}
                onDenoise={onStemDenoise}
              />

              <div className="flex items-center gap-1.5 font-mono text-xs tabular-nums text-muted-foreground">
                <Scissors weight="bold" className="size-3.5 text-primary-text" />
                <span className="text-foreground">{fmtClock(trimStart)}</span>
                <span>to</span>
                <span className="text-foreground">{fmtClock(trimEnd)}</span>
                <span className="text-muted-foreground/70">({fmtClock(selDuration)})</span>
              </div>

              <span className="flex-1" />

              {edited ? (
                <button
                  type="button"
                  onClick={() => {
                    setTouched(false);
                    setTrimStart(0);
                    setTrimEnd(duration);
                    setAudioEnabled(true);
                    setTrackCtl({});
                  }}
                  className="flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
                >
                  <ArrowCounterClockwise weight="bold" className="size-4" />
                  Reset
                </button>
              ) : null}

              <button
                type="button"
                disabled={!edited || clip.evicted}
                onClick={() => setSaveOpen(true)}
                title={
                  clip.evicted
                    ? "This clip is stored in the cloud only. Editing needs its local file."
                    : undefined
                }
                className="flex items-center gap-1.5 rounded-lg bg-primary px-5 py-1.5 text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <FloppyDisk weight="bold" className="size-4" />
                Save
              </button>
            </div>

            {/* Navigation hint — or, for an evicted clip, the download-to-edit
                affordance (re-fetch the cloud copy so trim/export can run). */}
            {clip.evicted ? (
              <div className="mt-3 flex flex-col items-center gap-2">
                {dl.downloading || download.isPending ? (
                  <div className="flex w-64 flex-col gap-1.5">
                    <div className="h-1.5 w-full overflow-hidden rounded-full bg-white/10">
                      <div
                        className="h-full rounded-full bg-primary transition-[width] duration-200"
                        style={{ width: `${Math.max(4, dl.pct)}%` }}
                      />
                    </div>
                    <p className="text-center text-xs text-muted-foreground/80">
                      Downloading from cloud… {Math.round(dl.pct)}%
                    </p>
                  </div>
                ) : (
                  <>
                    <button
                      type="button"
                      onClick={() => download.mutate(clip.id)}
                      className="flex items-center gap-1.5 rounded-lg bg-primary px-4 py-1.5 text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90"
                    >
                      <DownloadSimple weight="bold" className="size-4" />
                      Download to edit
                    </button>
                    <p className="max-w-sm text-center text-xs text-muted-foreground/70">
                      Cloud-only clip. Its local copy was freed up to save space.
                      Playing from the cloud; download it to trim or export.
                    </p>
                    {download.error ? (
                      <p className="text-center text-xs text-destructive">
                        {String(download.error)}
                      </p>
                    ) : null}
                  </>
                )}
              </div>
            ) : (
              <p className="mt-3 text-center text-xs text-muted-foreground/70">
                <Kbd>I</Kbd>/<Kbd>O</Kbd> set in/out · <Kbd>Space</Kbd> play ·{" "}
                <Kbd>←</Kbd> <Kbd>→</Kbd> browse · <Kbd>Del</Kbd> delete ·{" "}
                <Kbd>Esc</Kbd> close
              </p>
            )}
          </div>
        ) : null}
      </div>

      {/* ---- Details panel ---- */}
      <DetailsPanel
        clip={clip}
        onClose={onClose}
        onRename={onRename}
        onDelete={onDelete}
      />

      {/* ---- Save dialog ---- */}
      {saveOpen ? (
        <SaveDialog
          title={clip.title}
          selDuration={selDuration}
          audioSummary={
            !audioEnabled
              ? "audio removed"
              : (tracksEdited
                  ? audibleStems.length === 0
                    ? "all tracks muted"
                    : `${audibleStems.length} of ${stems.length} audio track${
                        stems.length === 1 ? "" : "s"
                      } mixed`
                  : hasStems
                    ? "all audio tracks kept"
                    : "audio kept") +
                // Note noise cancel when it's on for a stem that'll be heard.
                (audibleStems.some((s) => ctlOf(s.index).denoise)
                  ? " · noise cancelled"
                  : "")
          }
          pending={exportPending}
          error={exportError}
          onCancel={() => setSaveOpen(false)}
          onChoose={async (mode) => {
            // Overwriting replaces the file the webview has open — release it
            // first so the rename can't lose a fight with a Windows file lock.
            const v = videoRef.current;
            if (mode === "overwrite" && v) {
              v.pause();
              v.removeAttribute("src");
              v.load();
            }
            // A per-track mix re-muxes; otherwise it's a loss-less trim that
            // keeps every existing audio track.
            const tracks: TrackVolume[] | null =
              audioEnabled && hasStems && tracksEdited
                ? audibleStems.map((s) => ({
                    index: s.index,
                    volume: ctlOf(s.index).volume,
                    denoise: ctlOf(s.index).denoise,
                  }))
                : null;
            try {
              await onExport({
                start: trimStart,
                end: trimEnd,
                dropAudio: !audioEnabled,
                tracks,
                mode,
              });
              setSaveOpen(false);
              // On success an overwrite bumps size_bytes → the stage remounts
              // and reloads on its own; nothing else to do here.
            } catch {
              // Restore playback if the overwrite failed (error shown in dialog).
              // Only reachable for local clips (export is disabled when evicted),
              // so `src` is always a string here; coalesce to satisfy the type.
              if (mode === "overwrite" && v) {
                v.src = src ?? "";
                v.load();
              }
            }
          }}
        />
      ) : null}
    </div>
  );
}
