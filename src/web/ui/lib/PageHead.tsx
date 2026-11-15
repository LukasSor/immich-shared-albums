/** web/ui/lib/PageHead.tsx — the title every panel page opens with: the glyph, the name, and the household. See design-system.md. */
import { LinkIcon } from './LinkIcon.tsx';

export const PageHead = ({ household }: { household?: string }) => (
  <div class="isa-page-head">
    <span class="isa-page-icon">
      <LinkIcon />
    </span>
    <h1 class="isa-page-title">Shared albums</h1>
    {household && <span class="isa-page-household">· {household}</span>}
  </div>
);
