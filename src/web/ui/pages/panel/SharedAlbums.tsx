/** web/ui/pages/panel/SharedAlbums.tsx — the albums currently shared in either direction. See ../../../http-router.md. */
import { AlbumLink } from '../../lib/AlbumLink.tsx';
import { Icon } from '../../lib/icons.tsx';
import { Section } from '../../lib/Section.tsx';
import type { Album } from './api.ts';

export const SharedAlbums = ({ albums, onDenied }: { albums: Album[]; onDenied: () => void }) => (
  <Section title="Shared albums" icon="imageAlbum">
    {albums.length === 0 ? (
      <span class="isa-empty">None yet.</span>
    ) : (
      <div class="isa-rows">
        {albums.map(a => (
          <div class="isa-row" key={`${a.name}:${a.peer}:${a.role}`}>
            <span class="isa-row-icon">
              <Icon name="imageAlbum" size="22px" />
            </span>
            <div class="isa-row-main">
              <AlbumLink albumId={a.albumId} canOpen={a.canOpen} name={a.name} onDenied={onDenied} />
              <div class="isa-row-sub">
                {a.role === 'owner' ? 'shared out' : 'shared with us'} · {a.peer}
                {a.via === 'invite' ? ' · invited' : ''}
              </div>
            </div>
          </div>
        ))}
      </div>
    )}
  </Section>
);
