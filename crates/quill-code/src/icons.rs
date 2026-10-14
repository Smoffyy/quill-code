use gpui_kit::SharedString;

macro_rules! icons {
    ($($name:ident => $file:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon {
            $($name,)*
        }

        impl Icon {
            pub fn path(self) -> SharedString {
                match self {
                    $(Icon::$name => concat!("quill/icons/", $file, ".svg"),)*
                }
                .into()
            }
        }

        pub fn load(path: &str) -> Option<&'static [u8]> {
            $(
                if path == concat!("quill/icons/", $file, ".svg") {
                    return Some(include_bytes!(concat!("../../../assets/icons/", $file, ".svg")));
                }
            )*
            None
        }
    };
}

icons! {
    ArrowLeft => "arrow-left",
    ArrowUp => "up",
    Bolt => "quick-task",
    Branch => "fork",
    Bulb => "bulb",
    Chat => "chat",
    Check => "check",
    CheckCircle => "check-circle",
    ChevronDown => "chev-down",
    ChevronRight => "chevron",
    CircleAlert => "circle-alert",
    CircleStop => "circle-stop",
    CircleX => "circle-x",
    Code => "code",
    Commit => "git-commit",
    Compose => "compose",
    Copy => "copy",
    Cube => "cube",
    Dots => "dots",
    Download => "download",
    Eraser => "eraser",
    File => "paper",
    FileCode => "file-code",
    FileDiff => "file-diff",
    FilePen => "file-pen",
    FilePlus => "file-plus",
    FileText => "file-text",
    Files => "files",
    Folder => "folder",
    FolderOpen => "folder-open",
    FolderPlus => "folder-plus",
    FolderTree => "folder-tree",
    Gear => "gear",
    Globe => "globe",
    Image => "image-icon",
    Info => "info",
    Isolated => "isolated",
    LlamaCpp => "llama-cpp",
    Link => "link",
    Loader => "loader",
    Paperclip => "paperclip",
    Pencil => "pencil",
    Pin => "pin",
    Plus => "plus",
    PullRequest => "pull-request",
    Refresh => "refresh",
    Retry => "retry",
    Search => "search",
    Server => "server",
    Shield => "shield",
    Sliders => "sliders",
    Slash => "slash",
    Stop => "stop",
    Terminal => "terminal",
    Trash => "trash",
    X => "x",
}