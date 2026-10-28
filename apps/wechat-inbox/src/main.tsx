import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import Panel from './Panel';
import './styles.css';

const view = new URLSearchParams(window.location.search).get('view');
ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>{view === 'panel' ? <Panel /> : <App />}</React.StrictMode>,
);
