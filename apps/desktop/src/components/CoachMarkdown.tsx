import { applyCoachHardBreaks, formatCoachSummary } from '@ngx/shared';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { Components } from 'react-markdown';

const markdownComponents: Components = {
  a: ({ href, children, ...props }) => (
    <a href={href} target="_blank" rel="noopener noreferrer" {...props}>
      {children}
    </a>
  ),
  table: ({ children, ...props }) => (
    <div className="coach-md__table-wrap">
      <table {...props}>{children}</table>
    </div>
  ),
};

type CoachMarkdownProps = {
  content: string;
  className?: string;
};

export function CoachMarkdown({ content, className }: CoachMarkdownProps) {
  const cleaned = formatCoachSummary(content);
  if (!cleaned) return null;

  return (
    <div className={className ? `coach-md ${className}` : 'coach-md'}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
        {applyCoachHardBreaks(cleaned)}
      </ReactMarkdown>
    </div>
  );
}
