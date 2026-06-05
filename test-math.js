import { Editor } from '@tiptap/core';
import { Markdown } from 'tiptap-markdown';
import { MathExtension } from '@aarkue/tiptap-math-extension';
import StarterKit from '@tiptap/starter-kit';

const editor = new Editor({
  extensions: [StarterKit, Markdown.configure({ html: true }), MathExtension],
  content: '<p>Test <span data-type="inlineMath" data-latex="x^2"></span></p>'
});

console.log('Markdown output:', editor.storage.markdown.getMarkdown());
