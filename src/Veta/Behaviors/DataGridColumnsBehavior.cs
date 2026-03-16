using System.Collections.Generic;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Data;

namespace Veta.Behaviors;

public class DataGridColumnsBehavior
{
    public static readonly AttachedProperty<IEnumerable<string>?> ColumnNamesProperty =
        AvaloniaProperty.RegisterAttached<DataGridColumnsBehavior, DataGrid, IEnumerable<string>?>("ColumnNames");

    static DataGridColumnsBehavior()
    {
        ColumnNamesProperty.Changed.AddClassHandler<DataGrid>(OnColumnNamesChanged);
    }

    public static void SetColumnNames(DataGrid element, IEnumerable<string>? value) =>
        element.SetValue(ColumnNamesProperty, value);

    public static IEnumerable<string>? GetColumnNames(DataGrid element) =>
        element.GetValue(ColumnNamesProperty);

    private static void OnColumnNamesChanged(DataGrid grid, AvaloniaPropertyChangedEventArgs e)
    {
        grid.Columns.Clear();
        if (e.NewValue is not IEnumerable<string> columns) return;
        foreach (string col in columns)
            grid.Columns.Add(new DataGridTextColumn { Header = col, Binding = new Binding($"[{col}]") });
    }
}
