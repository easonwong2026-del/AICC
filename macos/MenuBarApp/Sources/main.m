#import <Cocoa/Cocoa.h>

@interface DashboardService : NSObject
@property(nonatomic, readonly) NSURL *rootURL;
@property(nonatomic, readonly) NSURL *logsURL;
@property(nonatomic, readonly) BOOL hasManagedProcess;
- (void)health:(void (^)(BOOL online, NSString *text))completion;
- (BOOL)start:(NSError **)error;
- (BOOL)stop:(NSError **)error;
- (BOOL)restart:(NSError **)error;
- (BOOL)enableLaunchAtLogin:(NSError **)error;
- (BOOL)disableLaunchAtLogin:(NSError **)error;
- (void)openPath:(NSString *)path;
- (NSDictionary<NSString *, NSString *> *)codexCacheSummary;
@end

@implementation DashboardService {
    NSTask *_managedTask;
    NSInteger _port;
    dispatch_queue_t _queue;
}

- (instancetype)init {
    self = [super init];
    if (!self) { return nil; }
    _queue = dispatch_queue_create("com.aieink.dashboard.menubar.service", DISPATCH_QUEUE_SERIAL);
    NSString *port = NSProcessInfo.processInfo.environment[@"EINK_PORT"];
    _port = port.length ? port.integerValue : 8765;
    _rootURL = [self resolveRootURL];
    _logsURL = [NSURL fileURLWithPath:[NSHomeDirectory() stringByAppendingPathComponent:@"Library/Logs/AI-EInk-Dashboard"] isDirectory:YES];
    return self;
}

- (NSURL *)resolveRootURL {
    NSURL *marker = [NSBundle.mainBundle URLForResource:@"ServerRoot" withExtension:@"txt"];
    if (marker) {
        NSString *path = [NSString stringWithContentsOfURL:marker encoding:NSUTF8StringEncoding error:nil];
        path = [path stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        if (path.length) { return [NSURL fileURLWithPath:path isDirectory:YES]; }
    }
    return [NSBundle.mainBundle.bundleURL URLByDeletingLastPathComponent].URLByDeletingLastPathComponent.URLByDeletingLastPathComponent;
}

- (BOOL)hasManagedProcess {
    __block BOOL running = NO;
    dispatch_sync(_queue, ^{
        running = self->_managedTask && self->_managedTask.isRunning;
    });
    return running;
}

- (void)health:(void (^)(BOOL online, NSString *text))completion {
    NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"http://127.0.0.1:%ld/api/health", (long)_port]];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:url cachePolicy:NSURLRequestReloadIgnoringLocalCacheData timeoutInterval:1.5];
        request.HTTPMethod = @"GET";
        dispatch_semaphore_t sema = dispatch_semaphore_create(0);
        __block NSData *data = nil;
        __block NSURLResponse *response = nil;
        __block NSError *error = nil;
        NSURLSessionDataTask *task = [NSURLSession.sharedSession dataTaskWithRequest:request completionHandler:^(NSData *taskData, NSURLResponse *taskResponse, NSError *taskError) {
            data = taskData;
            response = taskResponse;
            error = taskError;
            dispatch_semaphore_signal(sema);
        }];
        [task resume];
        if (dispatch_semaphore_wait(sema, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(2 * NSEC_PER_SEC))) != 0) {
            [task cancel];
            completion(NO, @"Offline: health check timed out");
            return;
        }
        NSHTTPURLResponse *http = (NSHTTPURLResponse *)response;
        if (error || ![http isKindOfClass:NSHTTPURLResponse.class] || http.statusCode != 200) {
            completion(NO, error ? [NSString stringWithFormat:@"Offline: %@", error.localizedDescription] : @"Offline: health check failed");
            return;
        }
        NSDictionary *json = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:nil] : nil;
        NSString *version = [json[@"version"] isKindOfClass:NSString.class] ? json[@"version"] : @"unknown";
        completion(YES, [NSString stringWithFormat:@"Online: v%@ · localhost:%ld", version, (long)self->_port]);
    });
}

- (BOOL)start:(NSError **)error {
    if ([self isHealthy]) { return YES; }
    NSTask *task = [self taskWithLaunchPath:@"/bin/bash" arguments:@[[self.rootURL.path stringByAppendingPathComponent:@"macos/start-dashboard.sh"]] logName:@"menubar-dashboard"];
    __block BOOL ok = YES;
    __block NSError *localError = nil;
    dispatch_sync(_queue, ^{
        @try {
            [task launch];
            self->_managedTask = task;
        } @catch (NSException *exception) {
            ok = NO;
            localError = [NSError errorWithDomain:@"AI-EInkDashboard" code:1 userInfo:@{NSLocalizedDescriptionKey: exception.reason ?: @"Failed to start service"}];
        }
    });
    if (!ok && error) { *error = localError; }
    return ok;
}

- (BOOL)stop:(NSError **)error {
    [self runLaunchPath:@"/bin/launchctl" arguments:@[@"bootout", [NSString stringWithFormat:@"gui/%u/com.aieink.dashboard", getuid()]] allowFailure:YES error:nil];
    dispatch_sync(_queue, ^{
        if (self->_managedTask.isRunning) { [self->_managedTask terminate]; }
        self->_managedTask = nil;
    });
    return YES;
}

- (BOOL)restart:(NSError **)error {
    [self stop:nil];
    [NSThread sleepForTimeInterval:0.4];
    return [self start:error];
}

- (BOOL)enableLaunchAtLogin:(NSError **)error {
    return [self runScript:@"macos/install-autostart.sh" error:error];
}

- (BOOL)disableLaunchAtLogin:(NSError **)error {
    return [self runScript:@"macos/uninstall-autostart.sh" error:error];
}

- (void)openPath:(NSString *)path {
    NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"http://127.0.0.1:%ld%@", (long)_port, path]];
    [NSWorkspace.sharedWorkspace openURL:url];
}

- (NSDictionary<NSString *, NSString *> *)codexCacheSummary {
    NSDictionary *codex = [self jsonAtPath:[self.rootURL.path stringByAppendingPathComponent:@"data/codex_last_success.json"]];
    if (![codex isKindOfClass:NSDictionary.class]) {
        NSDictionary *status = [self jsonAtPath:[self.rootURL.path stringByAppendingPathComponent:@"data/status.json"]];
        NSDictionary *fallback = status[@"codex"];
        codex = [fallback isKindOfClass:NSDictionary.class] ? fallback : nil;
    }
    if (![codex isKindOfClass:NSDictionary.class]) { return @{}; }

    NSDictionary *fiveHour = [codex[@"five_hour"] isKindOfClass:NSDictionary.class] ? codex[@"five_hour"] : nil;
    NSDictionary *weekly = [codex[@"weekly"] isKindOfClass:NSDictionary.class] ? codex[@"weekly"] : nil;
    NSNumber *fiveRemaining = [fiveHour[@"remaining"] isKindOfClass:NSNumber.class] ? fiveHour[@"remaining"] : nil;
    NSNumber *weeklyRemaining = [weekly[@"remaining"] isKindOfClass:NSNumber.class] ? weekly[@"remaining"] : nil;
    NSString *fiveReset = [fiveHour[@"reset"] isKindOfClass:NSString.class] ? fiveHour[@"reset"] : nil;
    NSString *weeklyReset = [weekly[@"reset"] isKindOfClass:NSString.class] ? weekly[@"reset"] : nil;

    NSMutableDictionary *summary = [NSMutableDictionary dictionary];
    if (fiveRemaining && weeklyRemaining) {
        summary[@"title"] = [NSString stringWithFormat:@"C %@/%@%%", [self percentText:fiveRemaining], [self percentText:weeklyRemaining]];
        summary[@"detail"] = [NSString stringWithFormat:@"CODEX 5h %@%% · weekly %@%%", [self percentText:fiveRemaining], [self percentText:weeklyRemaining]];
    } else if (fiveRemaining) {
        summary[@"title"] = [NSString stringWithFormat:@"C %@%%", [self percentText:fiveRemaining]];
        summary[@"detail"] = [NSString stringWithFormat:@"CODEX 5h %@%%", [self percentText:fiveRemaining]];
    } else if (weeklyRemaining) {
        summary[@"title"] = [NSString stringWithFormat:@"C %@%%", [self percentText:weeklyRemaining]];
        summary[@"detail"] = [NSString stringWithFormat:@"CODEX weekly %@%%", [self percentText:weeklyRemaining]];
    }

    NSMutableArray<NSString *> *resets = [NSMutableArray array];
    if (fiveReset.length) { [resets addObject:[NSString stringWithFormat:@"5h reset %@", fiveReset]]; }
    if (weeklyReset.length) { [resets addObject:[NSString stringWithFormat:@"weekly reset %@", weeklyReset]]; }
    if (resets.count) {
        NSString *detail = summary[@"detail"] ?: @"CODEX cache";
        summary[@"detail"] = [detail stringByAppendingFormat:@" · %@", [resets componentsJoinedByString:@" · "]];
    }
    return summary;
}

- (NSDictionary *)jsonAtPath:(NSString *)path {
    NSData *data = [NSData dataWithContentsOfFile:path];
    if (!data) { return nil; }
    id json = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    return [json isKindOfClass:NSDictionary.class] ? json : nil;
}

- (NSString *)percentText:(NSNumber *)value {
    double rounded = round(value.doubleValue);
    return [NSString stringWithFormat:@"%.0f", MAX(0, MIN(100, rounded))];
}

- (BOOL)runScript:(NSString *)relativePath error:(NSError **)error {
    NSString *script = [self.rootURL.path stringByAppendingPathComponent:relativePath];
    return [self runLaunchPath:@"/bin/bash" arguments:@[script] allowFailure:NO error:error];
}

- (BOOL)runLaunchPath:(NSString *)launchPath arguments:(NSArray<NSString *> *)arguments allowFailure:(BOOL)allowFailure error:(NSError **)error {
    NSTask *task = [self taskWithLaunchPath:launchPath arguments:arguments logName:@"menubar-actions"];
    @try {
        [task launch];
        [task waitUntilExit];
    } @catch (NSException *exception) {
        if (error) {
            *error = [NSError errorWithDomain:@"AI-EInkDashboard" code:1 userInfo:@{NSLocalizedDescriptionKey: exception.reason ?: @"Command failed"}];
        }
        return NO;
    }
    if (!allowFailure && task.terminationStatus != 0) {
        if (error) {
            *error = [NSError errorWithDomain:@"AI-EInkDashboard" code:task.terminationStatus userInfo:@{NSLocalizedDescriptionKey: [NSString stringWithFormat:@"Command failed: %@", [arguments componentsJoinedByString:@" "]]}];
        }
        return NO;
    }
    return YES;
}

- (NSTask *)taskWithLaunchPath:(NSString *)launchPath arguments:(NSArray<NSString *> *)arguments logName:(NSString *)logName {
    [NSFileManager.defaultManager createDirectoryAtURL:self.logsURL withIntermediateDirectories:YES attributes:nil error:nil];
    NSTask *task = [NSTask new];
    task.launchPath = launchPath;
    task.arguments = arguments;
    task.currentDirectoryPath = self.rootURL.path;
    task.environment = [self serviceEnvironment];
    task.standardOutput = [self logHandle:[NSString stringWithFormat:@"%@.log", logName]];
    task.standardError = [self logHandle:[NSString stringWithFormat:@"%@-error.log", logName]];
    return task;
}

- (NSFileHandle *)logHandle:(NSString *)fileName {
    NSURL *url = [self.logsURL URLByAppendingPathComponent:fileName];
    if (![NSFileManager.defaultManager fileExistsAtPath:url.path]) {
        [NSFileManager.defaultManager createFileAtPath:url.path contents:nil attributes:nil];
    }
    NSFileHandle *handle = [NSFileHandle fileHandleForWritingToURL:url error:nil];
    [handle seekToEndOfFile];
    return handle;
}

- (NSDictionary<NSString *, NSString *> *)serviceEnvironment {
    NSMutableDictionary *env = [NSProcessInfo.processInfo.environment mutableCopy];
    NSString *path = env[@"PATH"] ?: @"";
    env[@"PATH"] = [@[path, @"/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"] componentsJoinedByString:@":"];
    env[@"PYTHONDONTWRITEBYTECODE"] = @"1";
    return env;
}

- (BOOL)isHealthy {
    __block BOOL online = NO;
    dispatch_semaphore_t sema = dispatch_semaphore_create(0);
    [self health:^(BOOL value, NSString *text) {
        online = value;
        dispatch_semaphore_signal(sema);
    }];
    dispatch_semaphore_wait(sema, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(2 * NSEC_PER_SEC)));
    return online;
}

@end

@interface AppDelegate : NSObject <NSApplicationDelegate>
@end

@implementation AppDelegate {
    NSStatusItem *_statusItem;
    DashboardService *_service;
    NSTimer *_timer;
    NSMenuItem *_statusMenuItem;
    NSMenuItem *_startItem;
    NSMenuItem *_stopItem;
    NSMenuItem *_restartItem;
    NSMenuItem *_openSettingsItem;
    NSMenuItem *_openDashboardItem;
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    _service = [DashboardService new];
    [self configureMenu];
    [self startService];
    _timer = [NSTimer scheduledTimerWithTimeInterval:10 target:self selector:@selector(refreshStatus) userInfo:nil repeats:YES];
}

- (void)applicationWillTerminate:(NSNotification *)notification {
    [_timer invalidate];
}

- (void)configureMenu {
    _statusItem = [NSStatusBar.systemStatusBar statusItemWithLength:NSVariableStatusItemLength];
    _statusItem.button.title = @"AI E-Ink ...";
    _statusItem.button.image = [self menuBarIcon];
    _statusItem.button.imagePosition = NSImageLeft;
    _statusItem.button.toolTip = @"AI E-Ink Dashboard";

    NSMenu *menu = [NSMenu new];
    _statusMenuItem = [[NSMenuItem alloc] initWithTitle:@"Checking..." action:nil keyEquivalent:@""];
    _startItem = [self item:@"Start Service" action:@selector(startService) key:@"s"];
    _stopItem = [self item:@"Stop Service" action:@selector(stopService) key:@"x"];
    _restartItem = [self item:@"Restart Service" action:@selector(restartService) key:@"r"];
    _openSettingsItem = [self item:@"Open Settings" action:@selector(openSettings) key:@","];
    _openDashboardItem = [self item:@"Open Dashboard" action:@selector(openDashboard) key:@"d"];

    [menu addItem:_statusMenuItem];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItem:_startItem];
    [menu addItem:_stopItem];
    [menu addItem:_restartItem];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItem:_openSettingsItem];
    [menu addItem:_openDashboardItem];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItem:[self item:@"Enable Launch at Login" action:@selector(enableLaunchAtLogin) key:@""]];
    [menu addItem:[self item:@"Disable Launch at Login" action:@selector(disableLaunchAtLogin) key:@""]];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItem:[self item:@"Open Logs" action:@selector(openLogs) key:@"l"]];
    [menu addItem:[self item:@"Reveal Server Folder" action:@selector(revealServerFolder) key:@""]];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItem:[[NSMenuItem alloc] initWithTitle:@"Quit" action:@selector(terminate:) keyEquivalent:@"q"]];
    _statusItem.menu = menu;
}

- (NSMenuItem *)item:(NSString *)title action:(SEL)action key:(NSString *)key {
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:key];
    item.target = self;
    return item;
}

- (NSImage *)menuBarIcon {
    NSImage *image = [[NSImage alloc] initWithSize:NSMakeSize(18, 18)];
    [image lockFocus];
    [[NSColor clearColor] setFill];
    NSRectFill(NSMakeRect(0, 0, 18, 18));
    [[NSColor labelColor] setStroke];
    NSBezierPath *outer = [NSBezierPath bezierPathWithRoundedRect:NSMakeRect(2.5, 2.5, 13, 13) xRadius:2 yRadius:2];
    outer.lineWidth = 1.8;
    [outer stroke];
    for (NSInteger i = 0; i < 3; i++) {
        NSBezierPath *line = [NSBezierPath bezierPath];
        CGFloat y = 6 + i * 3;
        [line moveToPoint:NSMakePoint(5.2, y)];
        [line lineToPoint:NSMakePoint(12.8, y)];
        line.lineWidth = 1.6;
        [line stroke];
    }
    [image unlockFocus];
    image.template = YES;
    return image;
}

- (void)refreshStatus {
    [_service health:^(BOOL online, NSString *text) {
        dispatch_async(dispatch_get_main_queue(), ^{
            NSDictionary<NSString *, NSString *> *codex = [self->_service codexCacheSummary];
            NSString *codexTitle = codex[@"title"];
            NSString *codexDetail = codex[@"detail"];
            self->_statusItem.button.title = online ? (codexTitle ?: @"AI E-Ink: On") : @"AI E-Ink: Off";
            self->_statusMenuItem.title = online && codexDetail.length ? codexDetail : text;
            self->_startItem.enabled = !online;
            self->_stopItem.enabled = online || self->_service.hasManagedProcess;
            self->_restartItem.enabled = YES;
            self->_openSettingsItem.enabled = online;
            self->_openDashboardItem.enabled = online;
        });
    }];
}

- (void)runActionTitle:(NSString *)title block:(BOOL (^)(NSError **error))block {
    _statusMenuItem.title = title;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSError *error = nil;
        BOOL ok = block(&error);
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!ok) {
                self->_statusMenuItem.title = [NSString stringWithFormat:@"Error: %@", error.localizedDescription ?: @"Action failed"];
                return;
            }
            [self performSelector:@selector(refreshStatus) withObject:nil afterDelay:0.8];
        });
    });
}

- (void)startService { [self runActionTitle:@"Starting service..." block:^BOOL(NSError **error) { return [self->_service start:error]; }]; }
- (void)stopService { [self runActionTitle:@"Stopping service..." block:^BOOL(NSError **error) { return [self->_service stop:error]; }]; }
- (void)restartService { [self runActionTitle:@"Restarting service..." block:^BOOL(NSError **error) { return [self->_service restart:error]; }]; }
- (void)enableLaunchAtLogin { [self runActionTitle:@"Enabling launch at login..." block:^BOOL(NSError **error) { return [self->_service enableLaunchAtLogin:error]; }]; }
- (void)disableLaunchAtLogin { [self runActionTitle:@"Disabling launch at login..." block:^BOOL(NSError **error) { return [self->_service disableLaunchAtLogin:error]; }]; }
- (void)openSettings { [_service openPath:@"/settings"]; }
- (void)openDashboard { [_service openPath:@"/"]; }
- (void)openLogs { [NSWorkspace.sharedWorkspace openURL:_service.logsURL]; }
- (void)revealServerFolder { [NSWorkspace.sharedWorkspace activateFileViewerSelectingURLs:@[_service.rootURL]]; }

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        AppDelegate *delegate = [AppDelegate new];
        app.delegate = delegate;
        [app run];
    }
    return 0;
}
