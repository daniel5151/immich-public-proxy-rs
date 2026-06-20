import { useState, useEffect } from 'react';
import type { ShareDetails } from './types/generated/ShareDetails';
import { HomePage } from './pages/HomePage';
import { PasswordPage } from './pages/PasswordPage';
import { GalleryPage } from './pages/GalleryPage';

export default function App() {
  const [shareKey] = useState<string>(() => {
    const pathname = window.location.pathname;
    const parts = pathname.split('/').filter(Boolean);
    if ((parts[0] === 'share' || parts[0] === 's') && parts[1]) {
      return parts[1];
    }
    return '';
  });
  const [details, setDetails] = useState<ShareDetails | null>(null);
  const [loading, setLoading] = useState<boolean>(() => {
    const pathname = window.location.pathname;
    const parts = pathname.split('/').filter(Boolean);
    return !!((parts[0] === 'share' || parts[0] === 's') && parts[1]);
  });
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!shareKey) return;

    let active = true;
    const fetchDetails = async () => {
      try {
        const res = await fetch(`/api/share/${shareKey}`);
        if (!active) return;
        if (!res.ok) {
          const errData = await res.json().catch(() => ({}));
          throw new Error(errData.error || `Failed to fetch share details (${res.status})`);
        }
        const data: ShareDetails = await res.json();
        setDetails(data);
        setError(null);
      } catch (e) {
        if (active) {
          const message = e instanceof Error ? e.message : 'Failed to load share details';
          setError(message);
        }
      } finally {
        if (active) {
          setLoading(false);
        }
      }
    };

    fetchDetails();
    return () => {
      active = false;
    };
  }, [shareKey]);

  if (loading) {
    return (
      <div id="loading-spinner">
        <span className="loader"></span>
      </div>
    );
  }

  if (!shareKey) {
    return <HomePage />;
  }

  if (error) {
    return <div className="error-msg">Error: {error}</div>;
  }

  if (details?.passwordRequired) {
    return <PasswordPage shareKey={shareKey} />;
  }

  if (details) {
    return <GalleryPage details={details} />;
  }

  return <div className="error-msg">Error: Invalid share key</div>;
}
