module.exports = {
  content: ["./frontend/index.html", "./frontend/src/**/*.{ts,tsx}"],
  daisyui: {
    themes: [
      {
        awayuki: {
          primary: "#89b4fa",
          secondary: "#45475a",
          accent: "#f5c2e7",
          neutral: "#313244",
          "base-100": "#1e1e2e",
          "base-200": "#181825",
          "base-300": "#11111b",
          "base-content": "#cdd6f4",
          info: "#74c7ec",
          success: "#a6e3a1",
          warning: "#f9e2af",
          error: "#f38ba8",
          "--rounded-box": "0.375rem",
          "--rounded-btn": "0.25rem",
          "--rounded-badge": "0.25rem",
          "--animation-btn": "0.12s",
          "--animation-input": "0.12s",
          "--btn-text-case": "none",
          "--tab-radius": "0.25rem"
        }
      }
    ],
  },
  theme: {
    extend: {
      colors: {
        crust: "#11111b",
        mantle: "#181825",
        base: "#1e1e2e",
        surface0: "#313244",
        surface1: "#45475a",
        surface2: "#585b70",
        overlay0: "#6c7086",
        overlay1: "#7f849c",
        subtext0: "#a6adc8",
        text: "#cdd6f4",
        blue: "#89b4fa",
        sapphire: "#74c7ec",
        sky: "#89dceb",
        red: "#f38ba8",
        yellow: "#f9e2af",
        peach: "#fab387",
        green: "#a6e3a1"
      }
    }
  },
  plugins: [require("daisyui")],
};
