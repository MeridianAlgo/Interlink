import React, { useState } from 'react'
import { HashRouter as Router, Routes, Route, Link, useLocation } from 'react-router-dom'
import { motion, AnimatePresence } from 'framer-motion'
import { Zap, Github, Menu, X } from 'lucide-react'
import Home from './pages/Home'
import Documentation from './pages/Documentation'

const Navbar = () => {
  const [open, setOpen] = useState(false)
  const loc = useLocation()

  return (
    <>
      <header>
        <div className="container">
          <nav>
            <Link to="/" className="logo">
              <Zap size={16} />
              InterLink
            </Link>

            <ul className="nav-links desktop-only">
              <li><Link to="/" className={loc.pathname === '/' ? 'active' : ''}>Home</Link></li>
              <li><Link to="/docs" className={loc.pathname.startsWith('/docs') ? 'active' : ''}>Documentation</Link></li>
              <li>
                <a
                  href="https://github.com/MeridianAlgo/Cobalt"
                  target="_blank"
                  rel="noopener noreferrer"
                  className="nav-github"
                >
                  <Github size={13} />
                  GitHub
                </a>
              </li>
            </ul>

            <button
              className="mobile-only btn btn-ghost"
              style={{ padding: '0.4rem 0.6rem' }}
              onClick={() => setOpen(!open)}
              aria-label="Toggle menu"
            >
              {open ? <X size={18} /> : <Menu size={18} />}
            </button>
          </nav>
        </div>
      </header>

      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            style={{ position: 'fixed', top: 60, left: 0, right: 0, zIndex: 99 }}
            className="mobile-nav-drawer"
          >
            <Link to="/" onClick={() => setOpen(false)}>Home</Link>
            <Link to="/docs" onClick={() => setOpen(false)}>Documentation</Link>
            <a href="https://github.com/MeridianAlgo/Cobalt" target="_blank" rel="noopener noreferrer">GitHub</a>
          </motion.div>
        )}
      </AnimatePresence>
    </>
  )
}

const App = () => (
  <Router>
    <Navbar />
    <main>
      <Routes>
        <Route path="/" element={<Home />} />
        <Route path="/docs/*" element={<Documentation />} />
      </Routes>
    </main>
    <footer className="footer">
      <div className="container">
        <p>© 2026 MeridianAlgo Research Lab · InterLink Protocol · v0.6.1</p>
      </div>
    </footer>
  </Router>
)

export default App
