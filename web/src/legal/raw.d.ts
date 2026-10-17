// Text files imported as strings (Vite's `?raw`).
declare module '*.txt?raw' {
  const text: string;
  export default text;
}
