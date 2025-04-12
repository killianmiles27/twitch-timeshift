# Twichtimeshift2

**Twichtimeshift2** is a powerful Electron desktop app that lets you watch Twitch VODs as if you were watching them live — adjusted to your local time and offset preferences. Built over many iterations with core contributions from Killian Miles and development assistance from ChatGPT.

---

## 🧠 What It Does

Twichtimeshift2:
- Syncs Twitch VODs to your **local time** as if the stream started at the same hour in your timezone
- Lets you **choose your timezone** (Berlin, New York, Tokyo, etc.)
- Lets you apply a **time offset** (like +24 hours) so streams play exactly when you want them
- Automatically calculates how far into the VOD you are based on current time
- Opens the stream in a **locked Twitch player** (no scrubbing, no rewinding)
- Splits streamers into **Online** vs **Offline** with accurate start/end time info
- Lets you **add and remove streamers** via a simple popup menu
- Includes a **custom icon** (time-traveling capybara 🦫🕒)
- Fully GUI-powered — no command line needed

---

## 🤔 Why It Was Created

Like many Twitch fans, I was frustrated living in a different timezone from my favorite streamers. Streams often started while I was sleeping or busy, and catching up later never felt right — the experience was disconnected and full of spoilers.

So I built Twichtimeshift2 to bring the live stream feel back — by syncing VODs to your local time as if they just started.

### 🧪 Example Use Case:
A streamer goes live at **6 PM in Los Angeles**, but you live in **Berlin**. You use Twichtimeshift2 to simulate that stream starting at **6 PM Berlin time** the next day. If you open the app at **7:30 PM**, the VOD starts playing **90 minutes in**, just like a live experience synced to your day.

---

## 🚀 How It Works

1. The app checks your selected streamers using the Twitch API
2. It pulls their most recent VOD and finds the original **stream start time**
3. Converts that time into your **selected timezone**
4. Applies your chosen **hour offset** (e.g., +24 hours) to simulate delayed playback
5. If the current time is **within the simulated stream window**, it shows as ONLINE
6. Click “Watch” to open the embedded Twitch player at the correct timestamp

---

## 📦 How to Use

### 🧱 Setup Instructions

1. Download the `.7z` release file from the project’s GitHub Releases page
2. Extract it using [7-Zip](https://www.7-zip.org/)
3. Inside the extracted folder, run:
   ```
   Twichtimeshift2.exe
   ```

That’s it! No installation or setup needed. Your config will auto-save locally.

---

## 🖥 GUI Features

- Dropdowns for:
  - Major timezones
  - Offset options: `+0`, `+9`, `+12`, `+24`, `+33`
- Buttons:
  - “Refresh” to check stream status
  - “Add/Remove Streamers” opens a simple editor
- Auto-saves all settings in `config.json`

---

## 🔁 Versions & Iteration History

This app was developed across 20+ iterations, including:
- Fixing timezone logic
- Adding proper offset control
- Alphabetical sorting of streamers
- Adding/removing streamers dynamically
- Custom icon generation
- Full GUI redesign

---

## 👷‍♂️ Development Credit

- **Killian Miles** — Lead developer, UI/UX designer, and core architect of the app’s concept and workflow  
- **ChatGPT** — Development assistant: logic builder, refactorer, code debugger, and toolkit extender

> “Expect the worst, get the best.” — *Killian Miles*

---

## 📜 License

This project is licensed under **CC BY-NC-ND 4.0**  
Commercial use, redistribution, or modification of this app is not allowed.

---
