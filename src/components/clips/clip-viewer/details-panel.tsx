import * as React from "react";
import {
  X,
  PencilSimple,
  Copy,
  Check,
  Trash,
  Scissors,
  Lightning,
  FolderOpen,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import { useGameAssets } from "@/games/use-game-assets";
import { clipPresenter } from "@/games/clip-presenter";
import { revealClip } from "@/lib/api";
import type { ClipRecord } from "@/lib/api";
import { fmtDate } from "./format";
import { formatBytes, formatTime } from "@/lib/format";

function EditableTitle({ title, onCommit }: { title: string; onCommit: (title: string) => void }) {
  // `draft` doubles as the editing flag: null = not editing (render `title`
  // straight from the prop), a string = the working copy being edited. It's
  // seeded from `title` in the click handler, so no prop is copied into state on
  // mount and there's no re-sync effect that would flash a stale title.
  const [draft, setDraft] = React.useState<string | null>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const editing = draft !== null;

  React.useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  function commit() {
    const v = (draft ?? "").trim();
    setDraft(null);
    if (v && v !== title) onCommit(v);
  }

  if (editing) {
    return (
      <input
        ref={inputRef}
        value={draft ?? ""}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") setDraft(null);
        }}
        className="w-full rounded-md border border-border bg-field px-2.5 py-1.5 text-lg font-semibold outline-none focus:border-ring"
      />
    );
  }

  return (
    <button
      type="button"
      onClick={() => setDraft(title)}
      className="group/title flex items-start gap-2 text-left"
    >
      <span className="text-lg font-semibold leading-tight">{title || "Untitled"}</span>
      <PencilSimple className="mt-1 size-4 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover/title:opacity-100" />
    </button>
  );
}

/**
 * The right-hand details sidebar (title, spec line, event badges, match context,
 * file actions, delete). It depends only on `clip` + the action callbacks — none
 * of the player/editor state — so it's memoized: playing, scrubbing, mute,
 * speed, and trim edits no longer re-render it.
 */
export const DetailsPanel = React.memo(function DetailsPanel({
  clip,
  onClose,
  onRename,
  onDelete,
}: {
  clip: ClipRecord;
  onClose: () => void;
  onRename: (title: string) => void;
  onDelete: () => void;
}) {
  const trimmed = clip.event != null;
  return (
    <aside className="scrollbar-thin flex w-[340px] shrink-0 flex-col overflow-y-auto border-l border-panel-border bg-panel">
      <div className="flex items-center justify-between border-b border-panel-border px-5 py-4">
        <h2 className="text-sm font-semibold">Details</h2>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          className="flex size-8 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
        >
          <X weight="bold" className="size-4" />
        </button>
      </div>

      <div className="flex flex-1 flex-col gap-5 p-5">
        <EditableTitle title={clip.title} onCommit={onRename} />

        {/* One compact spec line — date, size, length, resolution */}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
          <span>{fmtDate(clip.created_unix_ms)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">{formatBytes(clip.size_bytes)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">{formatTime(clip.duration_secs)}</span>
          <span className="size-[3px] rounded-full bg-muted-foreground/40" />
          <span className="font-mono tabular-nums">
            {clip.width}×{clip.height}
          </span>
        </div>

        <div className="flex flex-wrap gap-2">
          {trimmed ? (
            // One badge per event the clip's window covered (a merged window
            // can hold several, e.g. a spike-defuse and a kill).
            (clip.events.length ? clip.events : [clip.event ?? ""]).map((ev, i) => (
              <span
                key={`${ev}-${i}`}
                className="inline-flex items-center gap-1.5 rounded-md bg-warning/15 px-2.5 py-1 text-xs font-medium text-warning"
              >
                <Scissors weight="fill" className="size-3.5" />
                {ev}
              </span>
            ))
          ) : (
            <span className="inline-flex items-center gap-1.5 rounded-md bg-info/15 px-2.5 py-1 text-xs font-medium text-info">
              <Lightning weight="fill" className="size-3.5" />
              Auto Clip
            </span>
          )}
        </div>

        {/* Valorant match context — silent for clips cut outside a match */}
        <ClipGameContext clip={clip} />

        <div className="flex flex-col gap-2">
          <span className="text-[11px] font-semibold tracking-wide text-muted-foreground/70 uppercase">
            File
          </span>
          <button
            type="button"
            onClick={() => {
              void revealClip(clip.id).catch(() => {});
            }}
            className="flex items-center justify-center gap-2 rounded-lg border border-border/60 bg-card/40 px-4 py-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
          >
            <FolderOpen className="size-4" />
            Open in folder
          </button>
          <CopyPath path={clip.path} />
        </div>

        <div className="mt-auto" />
        <button
          type="button"
          onClick={onDelete}
          className="flex items-center justify-center gap-2 rounded-lg border border-destructive/30 bg-destructive/10 px-4 py-2.5 text-sm font-medium text-destructive transition-colors hover:bg-destructive/20"
        >
          <Trash weight="bold" className="size-4" />
          Delete clip
        </button>
      </div>
    </aside>
  );
});

/**
 * Match context for the open clip — champion/agent, map, mode, result and K/D/A.
 * Game-aware: Valorant resolves agent artwork from valorant-api, League resolves
 * champion icons from Data Dragon. Renders nothing for clips cut outside a match
 * (all fields null), so the panel stays clean for non-match clips.
 */
function ClipGameContext({ clip }: { clip: ClipRecord }) {
  const assets = useGameAssets();
  const { icon, name, fallback, sub, showKda } = clipPresenter(clip).detail(clip, assets);
  const hasResult = clip.won != null;
  const hasKda = showKda && clip.kills != null && clip.deaths != null && clip.assists != null;

  if (!name && !sub && !hasResult && !hasKda) return null;

  return (
    <div className="flex flex-col gap-2">
      <span className="text-[11px] font-semibold tracking-wide text-muted-foreground/70 uppercase">
        Match
      </span>
      <div className="flex flex-col gap-3 rounded-lg border border-border/60 bg-card/40 p-3.5">
        <div className="flex items-center gap-3">
          {icon ? (
            <img
              src={icon}
              alt=""
              className="size-10 shrink-0 rounded-md bg-black/30 object-cover outline outline-1 -outline-offset-1 outline-white/10"
            />
          ) : null}
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold text-foreground">{name ?? fallback}</div>
            {sub ? <div className="truncate text-xs text-muted-foreground">{sub}</div> : null}
          </div>
          {hasResult ? (
            <span
              className={cn(
                "rounded-md px-2 py-0.5 text-[11px] font-bold text-white",
                clip.won ? "bg-success/80" : "bg-destructive/80",
              )}
            >
              {clip.won ? "WIN" : "LOSS"}
            </span>
          ) : null}
        </div>

        {hasKda ? (
          <div className="flex items-center gap-2 border-t border-border/50 pt-3 text-xs text-muted-foreground">
            <span className="font-mono tabular-nums">
              <span className="font-semibold text-foreground">{clip.kills}</span>
              {" / "}
              <span className="font-semibold text-foreground">{clip.deaths}</span>
              {" / "}
              <span className="font-semibold text-foreground">{clip.assists}</span>
            </span>
            <span className="text-muted-foreground/70">KDA</span>
            {clip.headshot_pct != null ? (
              <span className="ml-auto">
                <span className="font-mono font-semibold tabular-nums text-foreground">
                  {Math.round(clip.headshot_pct)}%
                </span>{" "}
                <span className="text-muted-foreground/70">HS</span>
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function CopyPath({ path }: { path: string }) {
  const [copied, setCopied] = React.useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  return (
    <button
      type="button"
      onClick={copy}
      className="flex items-center justify-center gap-2 rounded-lg border border-border/60 bg-card/40 px-4 py-2.5 text-sm font-medium text-muted-foreground transition-colors hover:text-foreground"
    >
      {copied ? <Check className="size-4 text-success" /> : <Copy className="size-4" />}
      {copied ? "Copied path" : "Copy file path"}
    </button>
  );
}

export function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="rounded border border-border/70 bg-card/60 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
      {children}
    </kbd>
  );
}
