// DemonOS: set the PhantomCore wallpaper on every desktop (runs once per user).
const all = desktops();
for (let i = 0; i < all.length; i++) {
    const d = all[i];
    d.wallpaperPlugin = "org.kde.image";
    d.currentConfigGroup = ["Wallpaper", "org.kde.image", "General"];
    d.writeConfig("Image", "file:///usr/share/wallpapers/DemonOS/contents/images/1920x1080.png");
}
