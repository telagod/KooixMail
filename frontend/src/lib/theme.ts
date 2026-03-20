export type Theme = "light" | "dark"

const STORAGE_KEY = "kooixmail-theme"

export function getSystemTheme(): Theme {
  if (typeof window === "undefined") return "light"
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
}

export function getStoredTheme(): Theme | null {
  try {
    const value = localStorage.getItem(STORAGE_KEY)
    if (value === "light" || value === "dark") return value
  } catch {
    // localStorage unavailable
  }
  return null
}

export function getInitialTheme(): Theme {
  return getStoredTheme() ?? getSystemTheme()
}

export function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme)
  try {
    localStorage.setItem(STORAGE_KEY, theme)
  } catch {
    // localStorage unavailable
  }
}

export function toggleTheme(): Theme {
  const current = document.documentElement.getAttribute("data-theme") as Theme
  const next: Theme = current === "dark" ? "light" : "dark"
  applyTheme(next)
  return next
}
