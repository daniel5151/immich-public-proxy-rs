import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

// Prevent Chrome intervention warnings when preventDefault() is called on non-cancelable events (like touchmove during scroll)
const originalPreventDefault = Event.prototype.preventDefault;
Event.prototype.preventDefault = function (this: Event) {
  if (this.cancelable !== false) {
    originalPreventDefault.call(this);
  }
};

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
