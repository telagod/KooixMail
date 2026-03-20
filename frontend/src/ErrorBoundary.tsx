import { Component, type ErrorInfo, type ReactNode } from "react"

interface Props {
  children: ReactNode
}

interface State {
  hasError: boolean
  error: Error | null
}

export class ErrorBoundary extends Component<Props, State> {
  constructor(props: Props) {
    super(props)
    this.state = { hasError: false, error: null }
  }

  static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("ErrorBoundary caught:", error, info.componentStack)
  }

  render() {
    if (this.state.hasError) {
      return (
        <div style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          minHeight: "100vh",
          padding: "2rem",
          fontFamily: "IBM Plex Sans, sans-serif",
          color: "#201913",
        }}>
          <h1 style={{ fontSize: "1.5rem", marginBottom: "1rem" }}>出了点问题</h1>
          <p style={{ color: "rgba(32,25,19,0.67)", marginBottom: "1.5rem" }}>
            {this.state.error?.message || "应用遇到了未知错误"}
          </p>
          <button
            onClick={() => window.location.reload()}
            style={{
              padding: "0.85rem 1.5rem",
              borderRadius: "999px",
              border: "none",
              background: "linear-gradient(135deg, #e47028, #f2a24a)",
              color: "white",
              fontWeight: 700,
              cursor: "pointer",
            }}
          >
            刷新页面
          </button>
        </div>
      )
    }

    return this.props.children
  }
}
