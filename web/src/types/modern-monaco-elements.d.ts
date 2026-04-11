declare namespace JSX {
  interface IntrinsicElements {
    'monaco-editor': React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> & {
      theme?: string;
      fontFamily?: string;
      fontSize?: string | number;
    };
  }
}
