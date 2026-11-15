/** web/ui/lib/Shell.tsx — the page chrome Immich's own pages have: a title bar with a way back, a breadcrumb and the page's actions. See design-system.md. */
import type { ComponentChildren } from 'preact';
import { Icon, type IconName } from './icons.tsx';

export type Crumb = { title: string; href?: string };
export type BarAction = { title: string; href: string; icon: IconName };

export const Shell = ({
  crumbs,
  household,
  actions = [],
  lede,
  children,
}: {
  crumbs: Crumb[];
  household?: string;
  actions?: BarAction[];
  lede?: ComponentChildren;
  children?: ComponentChildren;
}) => (
  <div class="isa-shell">
    <header class="isa-bar">
      <a class="isa-iconbtn" href="/" aria-label="Back to Immich" title="Back to Immich">
        <Icon name="arrowLeft" size="22px" />
      </a>
      <nav class="isa-crumbs" aria-label="Breadcrumb">
        {crumbs.map((crumb, index) => (
          <>
            {index > 0 && (
              <span class="isa-crumb-sep" aria-hidden="true">
                <Icon name="slashForward" size="1rem" />
              </span>
            )}
            {crumb.href ? (
              <a class="isa-crumb" href={crumb.href}>
                {crumb.title}
              </a>
            ) : (
              <span class="isa-crumb isa-crumb--current" aria-current="page">
                {crumb.title}
              </span>
            )}
          </>
        ))}
      </nav>
      {household && <span class="isa-bar-desc">{household}</span>}
      <div class="isa-bar-actions">
        {actions.map(action => (
          <a class="isa-btn isa-btn--text isa-bar-action" href={action.href} title={action.title}>
            <Icon name={action.icon} size="20px" />
            <span class="isa-bar-action-label">{action.title}</span>
          </a>
        ))}
      </div>
    </header>
    <main class="isa-main">
      {lede && <p class="isa-page-lede">{lede}</p>}
      {children}
    </main>
  </div>
);
