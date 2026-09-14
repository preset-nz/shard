import { Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import {
  addMap,
  type ControllersView,
  controllers,
  forgetControl,
  forgetDevice,
  type MappingsView,
  renameControl,
  renameDevice,
  renameMap,
  setActiveMap,
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
 * Right-click a device or a control to forget it. Stories 1 and 2 of
 * `controller-mapping.md`.
 *
 * Above the devices sits the map: which of the named, app-wide maps the
 * knobs play through. Switching arms every knob again.
 */
export function ControllersPanel({
  open,
  midi,
  onError,
}: {
  open: boolean;
  midi: MappingsView;
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

  const fail = (e: unknown) => onError(String(e));

  if (!view) return null;

  const mapRow = (
    <div className="flex items-center gap-2 px-3">
      <span className="shrink-0 text-xs text-muted-foreground">Map</span>
      {midi.active ? (
        <>
          <Select
            value={String(midi.active.id)}
            onValueChange={(id) => void setActiveMap(Number(id)).catch(fail)}
          >
            <SelectTrigger size="sm" className="h-6 w-36 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {midi.maps.map((m) => (
                <SelectItem key={m.id} value={String(m.id)} className="text-xs">
                  {m.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <NameField
            value={midi.active.name}
            onCommit={(n) => void renameMap(midi.active?.id ?? 0, n).catch(fail)}
          />
        </>
      ) : (
        <span className="flex-1 text-xs text-muted-foreground">
          None yet. Learn a control from a row and one is made.
        </span>
      )}
      <button
        type="button"
        className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        title="New map"
        onClick={() => void addMap(`map ${midi.maps.length + 1}`).catch(fail)}
      >
        <Plus className="size-3.5" />
      </button>
    </div>
  );

  if (view.devices.length === 0) {
    return (
      <div className="space-y-3">
        {mapRow}
        <p className="px-3 py-2 text-xs text-muted-foreground">
          No MIDI device seen yet. Plug one in and it appears here.
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      {mapRow}
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
