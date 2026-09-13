import { PropertyPanel } from '@preset.nz/facets';
import type { ParamRowContext } from '@/components/ParamRow';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { type ParamValues, scopeKeyFor } from '@/scope';

/**
 * Settings, on Cmd+,. Today it holds the tape's feel: how long the brake and a
 * reverse flick take (Georg, 2026-09-14). Brake and reverse themselves stay
 * on the header, because they are played.
 *
 * The values are still the patch's own. Roadmap row 4 makes them app-wide
 * defaults a patch can override, once the shared preferences store exists.
 */
export function SettingsDialog({
  open,
  onOpenChange,
  values,
  ctx,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  values: ParamValues;
  ctx: ParamRowContext;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            How the tape feels under the brake and reverse. Saved with the patch for now.
          </DialogDescription>
        </DialogHeader>
        <div className="-mx-3">
          <div className="px-3 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Tape
          </div>
          <PropertyPanel scopeKey={scopeKeyFor('tape')} selection={values} ctx={ctx} />
        </div>
      </DialogContent>
    </Dialog>
  );
}
