//! 購読の張り外しを包む型（[`Subscription`]）と、シグナルハンドラから外す手段
//! （[`unsubscribe_in_signal_handler`]）。

use std::fmt;
use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicI32, Ordering};

use super::{DISABLE_UPDATES, ENABLE_UPDATES, QUERY};

/// 配色の知らせ（モード 2031）の購読。張り外しを 1 つにまとめ、**落とせば外す**。
///
/// アプリが手で書いていた対を、この型が受け持つ:
///
/// | いつ | 呼ぶもの | 端末へ書くもの |
/// |---|---|---|
/// | 起動時の判定（OSC 11）の後。`--light` / `--dark` で固定なら呼ばない | [`start`] | `CSI ? 2031 h` |
/// | 子プロセスに端末を渡す前・Ctrl+Z で止まる前 | [`suspend`] | `CSI ? 2031 l` |
/// | 端末が戻ったとき | [`resume`] | `CSI ? 2031 h` `CSI ? 996 n` |
/// | 終わるとき（`?` での早い戻り・panic の巻き戻しも） | `Drop`（ほかの戻しとの順を決めたいなら [`stop`]） | `CSI ? 2031 l` |
/// | シグナルで終わるとき | ハンドラから [`unsubscribe_in_signal_handler`] | `CSI ? 2031 l` |
///
/// `resume` は今の配色も問い合わせる（離れていたあいだの切り替えは知らせが来ないので、答えで
/// 拾う。答えは知らせと同じ形で [`crate::input`] の読み手に届く）。起動時の `start` は
/// 問い合わせない（起動時の判定は OSC 11 のまま）。
///
/// 張っていないのに外す・端末を渡しているのに張る、ということはしない:
///
/// - `start` を呼んでいなければ（固定）、どれも何も書かない。[`Subscription::fixed`] は端末も
///   開かない「何もしない」形で、どこででも同じ呼び方ができる
/// - `suspend` の後は `resume` まで何も書かない。そのあいだに落ちても書かない（端末は子のもの。
///   子が自分で張った購読を外さない）。`suspend` 中の `start` は `resume` で張る
/// - 同じ呼び出しを重ねても 1 回と同じ
///
/// **外し忘れると**、次に端末を使うもの（Enter で開いた akapen、終わった後のシェル）に知らせが
/// 届き、crossterm で読むものはそこで止まる（[`crate::scheme`] の冒頭）。この型を、端末を
/// 戻す型（raw モード・代替画面を戻すもの）と同じ所に持つと外し忘れにくい。
///
/// ```no_run
/// use termtheme::scheme::Subscription;
///
/// # fn main() -> std::io::Result<()> {
/// # let fixed: Option<bool> = None;
/// crossterm::terminal::enable_raw_mode()?;
/// let light = fixed.unwrap_or_else(|| termtheme::background::detect_light().unwrap_or(false));
/// let mut scheme = Subscription::new();
/// if fixed.is_none() {
///     scheme.start()?; // 固定でなければ、開いたままの切り替えに追う
/// }
/// // …イベントループ。エディタを開くときは:
/// scheme.suspend()?;
/// // （raw モードを抜け、子を走らせ、raw モードに戻る）
/// scheme.resume()?;
/// // 落とせば外す（panic の巻き戻しでも）。
/// # let _ = light;
/// # Ok(())
/// # }
/// ```
///
/// # 書き先
///
/// - [`Subscription::new`]: 端末 — stdout が端末ならそこ、でなければ `/dev/tty`（stdout を
///   パイプにして TUI を `/dev/tty` に描くアプリと同じ選び方。開けなければ stdout）
/// - [`Subscription::stdout`]: stdout。ratatui の `CrosstermBackend<Stdout>` で描くアプリも
///   これでよい — `Stdout` の handle はどれも 1 つのバッファを共有するので、描画と順が崩れない
/// - [`Subscription::from_fd`]: 開いてある端末の fd（`/dev/tty` を自分で開いたとき、pty で
///   試すとき）
/// - [`Subscription::with_writer`]: 任意の `Write`。fd が分からないので、シグナルハンドラから
///   は書けない
///
/// 書くたびに flush する。描画と同じ端末へ別の handle で書くので、アプリの側で出力を溜めて
/// いるなら先に flush してから呼ぶ（stdout どうしなら要らない）。
///
/// # シグナル
///
/// シグナルハンドラの中では `Drop` が走らない。SIGINT・SIGTERM などで終わるときに外すには、
/// ハンドラから [`unsubscribe_in_signal_handler`] を呼ぶ。raw モードでは Ctrl+C・Ctrl+Z は
/// シグナルでなくキーとして届く。Ctrl+Z で止まるアプリは、`suspend` → `raise(SIGTSTP)` →
/// （`fg` で戻る）→ `resume` と呼ぶ（止まっているあいだ、シェルに知らせが届かない）。
///
/// `panic = "abort"` と `std::process::exit` でも `Drop` は走らない（そこで終わるなら、先に
/// [`stop`] を呼ぶ）。
///
/// プロセスに 1 つ持つ（シグナルハンドラが外すのは、最後に張ったものの書き先）。
///
/// [`start`]: Subscription::start
/// [`suspend`]: Subscription::suspend
/// [`resume`]: Subscription::resume
/// [`stop`]: Subscription::stop
#[must_use = "落とすと購読を外す（`let _ =` で受けるとすぐに外れる）"]
pub struct Subscription {
    out: Out,
    /// 購読したい（`start` を呼び、`stop` していない）。
    wanted: bool,
    /// 端末を持っている（`suspend` していない、あるいは `resume` した）。
    held: bool,
}

/// 書き先。
enum Out {
    /// 何も書かない（[`Subscription::fixed`]）。
    Nothing,
    Stdout,
    File(File),
    Writer(Box<dyn Write + Send>),
}

impl Out {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Nothing => Ok(()),
            Self::Stdout => {
                let mut out = io::stdout().lock();
                out.write_all(bytes)?;
                out.flush()
            }
            Self::File(file) => {
                file.write_all(bytes)?;
                file.flush()
            }
            Self::Writer(writer) => {
                writer.write_all(bytes)?;
                writer.flush()
            }
        }
    }

    /// シグナルハンドラが書ける fd（分からなければ `None`）。
    fn fd(&self) -> Option<RawFd> {
        match self {
            Self::Stdout => Some(libc::STDOUT_FILENO),
            Self::File(file) => Some(file.as_raw_fd()),
            Self::Nothing | Self::Writer(_) => None,
        }
    }
}

impl Subscription {
    /// 端末へ書く: stdout が端末ならそこ、でなければ `/dev/tty`（開けなければ stdout）。
    /// まだ張らない（[`Subscription::start`]）。
    pub fn new() -> Self {
        if io::stdout().is_terminal() {
            return Self::stdout();
        }
        match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            Ok(tty) => Self::with_out(Out::File(tty)),
            Err(_) => Self::stdout(),
        }
    }

    /// stdout へ書く。まだ張らない。
    pub fn stdout() -> Self {
        Self::with_out(Out::Stdout)
    }

    /// 開いてある端末の fd へ書く。まだ張らない。
    pub fn from_fd(fd: OwnedFd) -> Self {
        Self::with_out(Out::File(fd.into()))
    }

    /// 任意の `Write` へ書く。まだ張らない。**シグナルハンドラからは書けない**
    /// （[`unsubscribe_in_signal_handler`] は何もしない）。
    pub fn with_writer(writer: impl Write + Send + 'static) -> Self {
        Self::with_out(Out::Writer(Box::new(writer)))
    }

    /// 何もしない形（`--light` / `--dark` で固定したとき）。どれを呼んでも何も書かず、端末も
    /// 開かない。
    pub fn fixed() -> Self {
        Self::with_out(Out::Nothing)
    }

    fn with_out(out: Out) -> Self {
        Self {
            out,
            wanted: false,
            held: true,
        }
    }

    /// 購読を始める（`CSI ? 2031 h`）。起動時の判定（OSC 11）を終えてから呼ぶ。`suspend` 中なら
    /// 書かずに覚え、`resume` で張る。固定（[`Subscription::fixed`]）なら何もしない。
    pub fn start(&mut self) -> io::Result<()> {
        if self.wanted || matches!(self.out, Out::Nothing) {
            return Ok(());
        }
        self.wanted = true;
        if self.held {
            self.subscribe(ENABLE_UPDATES)
        } else {
            Ok(())
        }
    }

    /// 端末を手放す — 子プロセスに渡す前、Ctrl+Z で止まる前に呼ぶ。張っていれば外す
    /// （`CSI ? 2031 l`）。この後は [`Subscription::resume`] まで何も書かない（落ちても）。
    pub fn suspend(&mut self) -> io::Result<()> {
        if !self.held {
            return Ok(());
        }
        self.held = false;
        if self.wanted {
            self.unsubscribe()
        } else {
            Ok(())
        }
    }

    /// 端末が戻った — 購読していたなら張り直し、今の配色を問い合わせる（`CSI ? 2031 h`
    /// `CSI ? 996 n`）。答えは知らせと同じ形で読み手に届く。
    pub fn resume(&mut self) -> io::Result<()> {
        if self.held {
            return Ok(());
        }
        self.held = true;
        if self.wanted {
            self.subscribe(&[ENABLE_UPDATES, QUERY].concat())
        } else {
            Ok(())
        }
    }

    /// 購読をやめる。張っていれば外す（`CSI ? 2031 l`）。この後の `resume` は張り直さず、
    /// `Drop` は何もしない。端末を戻すほかのもの（代替画面・raw モード）との順を決めたいときに、
    /// `Drop` より先に呼ぶ。
    pub fn stop(&mut self) -> io::Result<()> {
        if !self.wanted {
            return Ok(());
        }
        self.wanted = false;
        if self.held {
            self.unsubscribe()
        } else {
            Ok(())
        }
    }

    /// 今、端末に購読を張っているか（`start` して、`suspend` も `stop` もしていない）。
    pub fn is_subscribed(&self) -> bool {
        self.wanted && self.held
    }

    /// 張る列を書く。シグナルハンドラに書き先を**先に**知らせる（書いた後だと、その間に
    /// 届いたシグナルで外し損ねる。外していないのに外す列を書くのは害が無い）。
    fn subscribe(&mut self, seq: &str) -> io::Result<()> {
        if let Some(fd) = self.out.fd() {
            SIGNAL_FD.store(fd, Ordering::SeqCst);
        }
        self.out.write(seq.as_bytes())
    }

    /// 外す列を書き、**書いてから**シグナルハンドラに知らせる。書けなくても（端末が閉じた）
    /// 張っていないことにする。
    fn unsubscribe(&mut self) -> io::Result<()> {
        let result = self.out.write(DISABLE_UPDATES.as_bytes());
        if let Some(fd) = self.out.fd() {
            let _ = SIGNAL_FD.compare_exchange(fd, -1, Ordering::SeqCst, Ordering::SeqCst);
        }
        result
    }
}

/// [`Subscription::new`] と同じ（端末へ書く。まだ張らない）。
impl Default for Subscription {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let out = match self.out {
            Out::Nothing => "fixed",
            Out::Stdout => "stdout",
            Out::File(_) => "fd",
            Out::Writer(_) => "writer",
        };
        f.debug_struct("Subscription")
            .field("out", &out)
            .field("wanted", &self.wanted)
            .field("held", &self.held)
            .finish()
    }
}

/// 購読を張っている書き先の fd（張っていなければ -1）。[`unsubscribe_in_signal_handler`] が読む。
static SIGNAL_FD: AtomicI32 = AtomicI32::new(-1);

/// **シグナルハンドラから呼ぶ**: [`Subscription`] が購読を張っているなら、その書き先へ
/// 外す列（`CSI ? 2031 l`）を書く。書いたら `true`。
///
/// async-signal-safe（原子的な読み 1 つと `write(2)` 1 回だけ。割り当ても端末を開くことも
/// しない）。張っていないとき — `start` 前、`suspend` 中（端末は子のもの）、`stop` の後、
/// 書き先の fd が分からないとき（[`Subscription::with_writer`]）— は何もしない。
///
/// ```no_run
/// extern "C" fn restore_and_die(sig: libc::c_int) {
///     termtheme::scheme::unsubscribe_in_signal_handler();
///     // （代替画面を出る列など、アプリのほかの戻しもここで write(2) する）
///     // SAFETY: 既定の扱いに戻して投げ直す（シェルが見る終わり方のまま死ぬ）。
///     unsafe {
///         libc::signal(sig, libc::SIG_DFL);
///         libc::raise(sig);
///     }
/// }
///
/// let handler: extern "C" fn(libc::c_int) = restore_and_die;
/// // SAFETY: restore_and_die はハンドラとして正しい形。
/// unsafe {
///     libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
///     libc::signal(libc::SIGINT, handler as libc::sighandler_t);
/// }
/// ```
///
/// ハンドラが書く列を自分で組むアプリ（ほかの戻しとまとめて 1 回で書く）は、
/// [`super::DISABLE_UPDATES`] を足してもよい。ただしそのときは、子プロセスに端末を渡して
/// いるあいだに書かないよう、アプリの側で見張る。
pub fn unsubscribe_in_signal_handler() -> bool {
    let fd = SIGNAL_FD.load(Ordering::SeqCst);
    if fd < 0 {
        return false;
    }
    // SAFETY: write(2) は async-signal-safe。列は 'static。
    unsafe {
        libc::write(
            fd,
            DISABLE_UPDATES.as_ptr().cast::<libc::c_void>(),
            DISABLE_UPDATES.len(),
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 書かれたバイトを後で見る書き先。
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl Written {
        /// 前に取り出してから書かれた分。
        fn take(&self) -> String {
            String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
        }
    }

    impl Write for Written {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    const ON: &str = "\x1b[?2031h";
    const OFF: &str = "\x1b[?2031l";
    const ASK: &str = "\x1b[?996n";

    fn subscription() -> (Subscription, Written) {
        let written = Written::default();
        (Subscription::with_writer(written.clone()), written)
    }

    #[test]
    fn start_suspend_resume_and_drop_write_the_pairs() {
        let (mut s, w) = subscription();
        assert_eq!(w.take(), "", "作っただけでは張らない");
        s.start().unwrap();
        assert_eq!(w.take(), ON, "起動時は問い合わせない");
        assert!(s.is_subscribed());
        s.suspend().unwrap();
        assert_eq!(w.take(), OFF);
        assert!(!s.is_subscribed());
        s.resume().unwrap();
        assert_eq!(w.take(), format!("{ON}{ASK}"), "張り直してから問い合わせる");
        assert!(s.is_subscribed());
        drop(s);
        assert_eq!(w.take(), OFF, "落とせば外す");
    }

    #[test]
    fn nothing_is_written_unless_started() {
        // 固定（start を呼ばない）なら、手放しても戻っても落としても書かない。
        let (mut s, w) = subscription();
        s.suspend().unwrap();
        s.resume().unwrap();
        s.stop().unwrap();
        assert!(!s.is_subscribed());
        drop(s);
        assert_eq!(w.take(), "");
        // 何もしない形は start も効かない。
        let mut fixed = Subscription::fixed();
        fixed.start().unwrap();
        fixed.suspend().unwrap();
        fixed.resume().unwrap();
        assert!(!fixed.is_subscribed());
    }

    #[test]
    fn repeated_calls_are_the_same_as_one() {
        let (mut s, w) = subscription();
        s.start().unwrap();
        s.start().unwrap();
        assert_eq!(w.take(), ON);
        s.resume().unwrap();
        assert_eq!(w.take(), "", "持っている端末には張り直さない");
        s.suspend().unwrap();
        s.suspend().unwrap();
        assert_eq!(w.take(), OFF);
        s.resume().unwrap();
        s.resume().unwrap();
        assert_eq!(w.take(), format!("{ON}{ASK}"));
    }

    #[test]
    fn nothing_is_written_while_the_terminal_is_handed_over() {
        // 手放しているあいだに落ちても書かない（端末は子のもの。外すのは suspend で済んでいる）。
        let (mut s, w) = subscription();
        s.start().unwrap();
        s.suspend().unwrap();
        w.take();
        drop(s);
        assert_eq!(w.take(), "");
        // 手放しているあいだの start は、戻ったときに張る。
        let (mut s, w) = subscription();
        s.suspend().unwrap();
        s.start().unwrap();
        assert_eq!(w.take(), "");
        assert!(!s.is_subscribed());
        s.resume().unwrap();
        assert_eq!(w.take(), format!("{ON}{ASK}"));
        // stop した後は、手放して戻っても張り直さない。
        let (mut s, w) = subscription();
        s.start().unwrap();
        s.suspend().unwrap();
        s.stop().unwrap();
        s.resume().unwrap();
        assert_eq!(w.take(), format!("{ON}{OFF}"));
    }

    #[test]
    fn stop_unsubscribes_once_before_drop() {
        let (mut s, w) = subscription();
        s.start().unwrap();
        s.stop().unwrap();
        assert_eq!(w.take(), format!("{ON}{OFF}"));
        assert!(!s.is_subscribed());
        drop(s);
        assert_eq!(w.take(), "", "stop の後の Drop は書かない");
    }

    #[test]
    fn a_panic_unwinding_through_the_owner_unsubscribes() {
        let (mut s, w) = subscription();
        s.start().unwrap();
        w.take();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned = s;
            panic!("イベントループの中の panic");
        }));
        assert!(result.is_err());
        assert_eq!(w.take(), OFF);
    }

    #[test]
    fn a_writer_without_an_fd_is_not_touched_by_the_signal_handler() {
        let (mut s, _w) = subscription();
        s.start().unwrap();
        assert!(!unsubscribe_in_signal_handler(), "書き先の fd が分からない");
    }

    #[test]
    fn debug_shows_the_state() {
        let (mut s, _w) = subscription();
        s.start().unwrap();
        assert_eq!(
            format!("{s:?}"),
            "Subscription { out: \"writer\", wanted: true, held: true }"
        );
    }
}
