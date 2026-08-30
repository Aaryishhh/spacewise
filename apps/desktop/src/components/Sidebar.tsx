import { NavLink } from "react-router-dom";
import "./Sidebar.css";

const LINKS = [
  { to: "/", label: "Overview" },
  { to: "/storage", label: "Storage" },
  { to: "/cleanup", label: "Cleanup" },
  { to: "/large-files", label: "Large Files" },
  { to: "/duplicates", label: "Duplicates" },
  { to: "/applications", label: "Applications" },
  { to: "/developer", label: "Developer" },
  { to: "/history", label: "History" },
  { to: "/settings", label: "Settings" },
];

export default function Sidebar() {
  return (
    <nav className="sidebar">
      <div className="sidebar-title">Spacewise</div>
      <ul>
        {LINKS.map((link) => (
          <li key={link.to}>
            <NavLink to={link.to} end={link.to === "/"} className={({ isActive }) => (isActive ? "active" : "")}>
              {link.label}
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  );
}
