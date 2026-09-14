import { useEffect, useState } from 'react';
import {
  type ControllersView,
  controllers,
  forgetControl,
  forgetDevice,
  renameControl,
  renameDevice,
  setControlRole,
} from '@/audio';
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

/**
 * Settings → Controllers. Every MIDI port that has been seen, and every knob
 * or pad that has been touched, listed as it was learned. Nothing is mapped
 * here; this is where a control gets its name and where you check that it
 * is arriving at all: the dot beside a control lights while it moves.
 *
 * Right-click a device or a control to forget it. Story 1 of
 * `controller-mapping.md`.
 */
export function ControllersPanel({
  open,
  onError,
}: {
  open: boolean;
  onError: (message: string) => void;
}) {
  const [view, setView] = useState<ControllersView | null>(null);

  // Poll only while visible. Ten times a second is enough for a dot.
  useEffect(() => {
    if (!open) return;
    let alive = true;
    const tick = () =>
      void controllers()
        .then((v) => {
          if (alive) setView(v);
        })
        .catch(() => {});
    tick();
    const id = setInterval(tick, 100);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [open]);

  const apply = (p: Promise<ControllersView>) =>
    void p.then(setView).catch((e) => onError(String(e)));

  if (!view) return null;
  if (view.devices.length === 0) {
    return (
      <p className="px-3 py-2 text-xs text-muted-foreground">
        No MIDI device seen yet. Plug one in and it appears here.
      </p>
    );
  }

  return (
    <div className="space-y-3">
      {view.devices.map((d) => {
        const controls = view.controls.filter((c) => c.device === d.port);
        return (
          <div key={d.port}>
            <ContextMenu>
              <ContextMenuTrigger asChild>
                <div className="flex items-center gap-2 px-3">
                  <span
                    className={
                      d.connected
                        ? 'size-1.5 rounded-full bg-emerald-400'
                        : 'size-1.5 rounded-full bg-muted-foreground/40'
                    }
                    title={d.connected ? 'Connected' : 'Not connected'}
                  />
                  <NameField value={d.name} onCommit={(n) => apply(renameDevice(d.port, n))} />
                  <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
                    {d.port}
                  </span>
                </div>
              </ContextMenuTrigger>
              <ContextMenuContent>
                <ContextMenuItem onSelect={() => apply(forgetDevice(d.port))}>
                  Forget device and its controls
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
            {controls.length === 0 ? (
              <p className="px-3 py-1 text-xs text-muted-foreground">
                Touch a knob or pad and it appears here.
              </p>
            ) : (
              <div className="mt-1">
                {controls.map((c) => (
                  <ContextMenu key={c.id}>
                    <ContextMenuTrigger asChild>
                      <div className="flex items-center gap-2 px-3 py-0.5">
                        <span
                          className={
                            c.active
                              ? 'size-1.5 rounded-full bg-amber-400'
                              : 'size-1.5 rounded-full bg-muted-foreground/25'
                          }
                        />
                        <NameField value={c.name} onCommit={(n) => apply(renameControl(c.id, n))} />
                        <Select
                          value={c.role}
                          onValueChange={(r) => apply(setControlRole(c.id, r))}
                        >
                          <SelectTrigger size="sm" className="h-6 w-20 text-xs">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {view.roles.map((r) => (
                              <SelectItem key={r} value={r} className="text-xs">
                                {r}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <span className="w-24 shrink-0 text-right font-mono text-[10px] text-muted-foreground">
                          ch {c.channel + 1} {c.kind} {c.number}
                        </span>
                        <span className="w-7 shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
                          {c.last ?? ''}
                        </span>
                      </div>
                    </ContextMenuTrigger>
                    <ContextMenuContent>
                      <ContextMenuItem onSelect={() => apply(forgetControl(c.id))}>
                        Forget control
                      </ContextMenuItem>
                    </ContextMenuContent>
                  </ContextMenu>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** A name that commits on blur or Enter, and reverts on Escape. */
function NameField({ value, onCommit }: { value: string; onCommit: (name: string) => void }) {
  const [draft, setDraft] = useState(value);
  const [editing, setEditing] = useState(false);
  // While not editing, follow the value from Rust.
  useEffect(() => {
    if (!editing) setDraft(value);
  }, [value, editing]);
  return (
    <Input
      className="h-6 flex-1 px-1.5 text-xs"
      value={draft}
      onFocus={() => setEditing(true)}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        setEditing(false);
        if (draft.trim() !== value) onCommit(draft);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
        if (e.key === 'Escape') {
          setDraft(value);
          setEditing(false);
          (e.target as HTMLInputElement).blur();
        }
      }}
    />
  );
}
