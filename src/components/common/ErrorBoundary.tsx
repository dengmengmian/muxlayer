import { Component, type ErrorInfo, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle } from "lucide-react";
import { useI18n } from "@/lib/i18n";

interface Props {
  children: ReactNode;
  t: (key: string) => string;
}

interface State {
  error: Error | null;
}

/// 页面级错误边界：某个路由页渲染抛错时只替换内容区，侧边栏 / 顶栏保持可用，
/// 避免整棵树被卸载出现白屏。AppShell 用 pathname 做 key，切路由自动复位；
/// 同一路由（如 "/" 本身出错）没法靠切路由复位，所以另给一个"重试"清空错误。
class ErrorBoundaryInner extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  retry = () => this.setState({ error: null });

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Page render failed", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const { t } = this.props;
    return (
      <div
        role="alert"
        className="flex flex-col items-start gap-3 rounded-lg border border-error/20 bg-error-soft p-6"
      >
        <div className="flex items-center gap-2 text-sm font-medium text-text-primary">
          <AlertTriangle className="h-4 w-4 text-error" />
          {t("error_boundary.title")}
        </div>
        <pre className="max-w-full whitespace-pre-wrap break-words font-mono text-xs text-text-secondary">
          {error.message || String(error)}
        </pre>
        <div className="flex items-center gap-4">
          <button
            type="button"
            onClick={this.retry}
            className="text-xs text-accent underline-offset-2 hover:underline"
          >
            {t("error_boundary.retry")}
          </button>
          <Link
            to="/"
            className="text-xs text-accent underline-offset-2 hover:underline"
          >
            {t("error_boundary.back")}
          </Link>
        </div>
      </div>
    );
  }
}

export function ErrorBoundary({ children }: { children: ReactNode }) {
  const { t } = useI18n();
  return <ErrorBoundaryInner t={t}>{children}</ErrorBoundaryInner>;
}
