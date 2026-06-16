import { useEffect, useState } from "react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { useSettings, useUpdateSettings } from "@/hooks/use-settings";
import type { EventToggles, Settings } from "@/lib/api";

const EVENT_LABELS: { key: keyof EventToggles; label: string }[] = [
  { key: "kill", label: "Kill" },
  { key: "double_kill", label: "2K" },
  { key: "triple_kill", label: "3K" },
  { key: "quadra_kill", label: "4K" },
  { key: "ace", label: "Ace" },
  { key: "knife", label: "Knife" },
  { key: "death", label: "Death" },
  { key: "assist", label: "Assist" },
];

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex items-center justify-between gap-4 text-sm">
      <span className="text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

const inputCls =
  "w-32 rounded-md border border-input bg-background px-2 py-1 text-sm";

export default function SettingsPage() {
  const { data, isLoading } = useSettings();
  const update = useUpdateSettings();
  const [draft, setDraft] = useState<Settings | null>(null);

  useEffect(() => {
    if (data) setDraft(data);
  }, [data]);

  if (isLoading || !draft) {
    return <div className="p-8 text-sm text-muted-foreground">Loading settings…</div>;
  }

  const set = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setDraft({ ...draft, [key]: value });

  const toggleEvent = (key: keyof EventToggles) =>
    setDraft({ ...draft, events: { ...draft.events, [key]: !draft.events[key] } });

  return (
    <div className="space-y-6 p-8">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
          <p className="text-sm text-muted-foreground">
            Persisted to <code>settings.json</code> in the app config dir.
          </p>
        </div>
        <Button onClick={() => update.mutate(draft)} disabled={update.isPending}>
          {update.isPending ? "Saving…" : "Save"}
        </Button>
      </header>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="text-base">Capture & encoder</CardTitle>
            <CardDescription>FPS, codec, bitrate ceiling.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Field label="Target FPS">
              <select
                className={inputCls}
                value={draft.target_fps}
                onChange={(e) => set("target_fps", Number(e.target.value))}
              >
                {[30, 60, 120, 144, 240].map((f) => (
                  <option key={f} value={f}>
                    {f}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Codec">
              <select
                className={inputCls}
                value={draft.codec}
                onChange={(e) => set("codec", e.target.value)}
              >
                {["h264", "hevc", "av1"].map((c) => (
                  <option key={c} value={c}>
                    {c.toUpperCase()}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Bitrate (Mbps)">
              <input
                type="number"
                className={inputCls}
                value={draft.bitrate_mbps}
                onChange={(e) => set("bitrate_mbps", Number(e.target.value))}
              />
            </Field>
            <Field label="Capture audio">
              <input
                type="checkbox"
                checked={draft.capture_audio}
                onChange={(e) => set("capture_audio", e.target.checked)}
              />
            </Field>
            <Field label="Capture mode">
              <select
                className={inputCls}
                value={draft.capture_mode === "hook" ? "hook" : "wgc"}
                onChange={(e) => set("capture_mode", e.target.value)}
              >
                <option value="wgc">WGC (safe, default)</option>
                <option value="hook">Game hook (high FPS)</option>
              </select>
            </Field>
            {draft.capture_mode === "hook" ? (
              <p className="rounded-md border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
                ⚠️ Game-hook mode injects into the game to capture above the
                ~60&nbsp;FPS desktop-composition cap. Anti-cheats (e.g. Valorant's
                Vanguard) may flag the injector and put your account at risk. Use
                at your own risk; WGC stays the safe default.
              </p>
            ) : null}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="text-base">Buffer & hotkey</CardTitle>
            <CardDescription>RAM ring depth, clip padding, save key.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Field label="Buffer seconds">
              <input
                type="number"
                className={inputCls}
                value={draft.buffer_seconds}
                onChange={(e) => set("buffer_seconds", Number(e.target.value))}
              />
            </Field>
            <Field label="Pad before (s)">
              <input
                type="number"
                className={inputCls}
                value={draft.pad_before_secs}
                onChange={(e) => set("pad_before_secs", Number(e.target.value))}
              />
            </Field>
            <Field label="Pad after (s)">
              <input
                type="number"
                className={inputCls}
                value={draft.pad_after_secs}
                onChange={(e) => set("pad_after_secs", Number(e.target.value))}
              />
            </Field>
            <Field label="Save hotkey">
              <input
                type="text"
                className={inputCls}
                value={draft.save_hotkey}
                onChange={(e) => set("save_hotkey", e.target.value)}
              />
            </Field>
          </CardContent>
        </Card>

        <Card className="md:col-span-2">
          <CardHeader>
            <CardTitle className="text-base">Valorant auto-clip events</CardTitle>
            <CardDescription>
              Which highlights are auto-clipped.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-wrap gap-4">
            {EVENT_LABELS.map(({ key, label }) => (
              <label key={key} className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={draft.events[key]}
                  onChange={() => toggleEvent(key)}
                />
                {label}
              </label>
            ))}
          </CardContent>
        </Card>
      </div>

      {update.error ? (
        <p className="text-sm text-destructive">{String(update.error)}</p>
      ) : null}
    </div>
  );
}
