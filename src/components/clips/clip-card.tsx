import * as React from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Link } from "@tanstack/react-router";
import {
  Triangle,
  Monitor,
  Lightning,
  Scissors,
  Copy,
  Check,
  CloudArrowUp,
  Play,
  DotsThree,
  PencilSimple,
  Trash,
} from "@phosphor-icons/react";

import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { ClipRecord } from "@/lib/api";

function fmtDuration(secs: number): string {
  const s = Math.round(secs);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function fmtSize(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

function timeAgo(unixMs: number): string {
  const diff = Date.now() - unixMs;
  const mins = Math.round(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours} hour${hours > 1 ? "s" : ""} ago`;
  const days = Math.round(hours / 24);
  return `${days} day${days > 1 ? "s" : ""} ago`;
}

function Dot() {
  return <span className="size-[3px] shrink-0 rounded-full bg-secondary" />;
}

export function ClipCard({
  clip,
  onDelete,
  onRename,
}: {
  clip: ClipRecord;
  onDelete: () => void;
  onRename: () => void;
}) {
  const [copied, setCopied] = React.useState(false);
  const trimmed = clip.event != null;

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(convertFileSrc(clip.path));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }

  return (
    <div className="group flex flex-col overflow-hidden rounded-xl border border-border/60 bg-card shadow-sm transition-colors hover:border-border">
      {/* Thumbnail */}
      <Link
        to="/clips/$clipId"
        params={{ clipId: String(clip.id) }}
        className="relative block aspect-video overflow-hidden bg-muted"
      >
        {clip.thumb_path ? (
          <img
            src={convertFileSrc(clip.thumb_path)}
            alt={clip.title}
            className="size-full object-cover opacity-90 transition-all duration-300 group-hover:scale-[1.02] group-hover:opacity-100"
            onError={(e) => {
              (e.currentTarget as HTMLImageElement).style.display = "none";
            }}
          />
        ) : null}
        <span className="absolute inset-0 bg-gradient-to-t from-black/50 to-transparent opacity-60" />

        {/* Hover play affordance */}
        <span className="absolute inset-0 flex items-center justify-center opacity-0 transition-opacity group-hover:opacity-100">
          <span className="flex size-11 items-center justify-center rounded-full bg-black/55 backdrop-blur-sm">
            <Play weight="fill" className="size-5 text-white" />
          </span>
        </span>

        <span className="absolute right-2 bottom-2 rounded bg-black/80 px-1.5 py-0.5 text-[10px] font-medium text-white backdrop-blur-sm">
          {fmtDuration(clip.duration_secs)}
        </span>
      </Link>

      {/* Meta */}
      <div className="flex flex-1 flex-col gap-1.5 p-3.5">
        <div className="flex items-center justify-between gap-2">
          <h3
            className="truncate text-sm font-medium text-card-foreground"
            title={clip.title}
          >
            {clip.title || "Untitled"}
          </h3>

          <DropdownMenu>
            <DropdownMenuTrigger
              aria-label="Clip actions"
              className="-mr-1 flex size-6 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-[color,opacity] outline-none hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
            >
              <DotsThree weight="bold" className="size-4" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={onRename}>
                <PencilSimple />
                Rename
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={copyLink}>
                <Copy />
                Copy link
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={onDelete}>
                <Trash />
                Delete
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        <div className="flex items-center gap-1.5 truncate text-[11px] font-medium text-muted-foreground">
          <span className="flex size-3 items-center justify-center rounded-sm bg-primary/20">
            <Triangle weight="fill" className="size-2 text-primary" />
          </span>
          <Dot />
          <Monitor className="size-3" />
          <span>On Device</span>
          <Dot />
          <span>{fmtSize(clip.size_bytes)}</span>
          <Dot />
          {trimmed ? (
            <Scissors className="size-3 text-warning" />
          ) : (
            <Lightning weight="fill" className="size-3 text-info" />
          )}
          <span className="truncate">{timeAgo(clip.created_unix_ms)}</span>
        </div>
      </div>

      {/* Actions */}
      <div className="mt-auto flex items-center justify-between border-t border-border/60 px-3.5 pt-2 pb-3.5 text-muted-foreground">
        <button
          type="button"
          onClick={copyLink}
          className="flex items-center gap-1.5 text-xs font-medium transition-colors hover:text-foreground"
        >
          {copied ? (
            <Check className="size-3.5 text-success" />
          ) : (
            <Copy className="size-3.5" />
          )}
          {copied ? "Copied" : "Copy Link"}
        </button>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label="Upload to cloud"
              className={cn("transition-colors hover:text-foreground")}
            >
              <CloudArrowUp className="size-4" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Upload to cloud</TooltipContent>
        </Tooltip>
      </div>
    </div>
  );
}
